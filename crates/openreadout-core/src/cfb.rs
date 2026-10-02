//! A reader for compound files (OLE2 structured storage), written from Microsoft's public
//! "[MS-CFB]: Compound File Binary File Format" open specification: header, FAT/DIFAT, mini
//! FAT, directory tree and stream reads. Shared by the readers of compound-file formats
//! (Shimadzu `.lcd`/`.gcd`, Zeiss `.zvi`); errors carry the calling format's id.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::bytes::{le_u16, le_u32, read_range, utf16le_z};
use crate::error::{Error, Result};

/// First eight bytes of every compound file.
pub const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// Longest chain followed before a file is declared corrupt (loops).
const MAX_CHAIN: usize = 1 << 24;

/// A directory entry: a storage (folder) or a stream (file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfbEntry {
    /// Index in the directory.
    pub id: u32,
    /// Path from the root, `/`-separated (the root storage is `""`).
    pub path: String,
    /// True for streams, false for storages.
    pub is_stream: bool,
    /// Stream size in bytes (0 for storages).
    pub size: u64,
    /// First sector of the stream (in the mini stream when `size` is below the mini cutoff).
    pub start_sector: u32,
    /// Modification time as a Windows FILETIME (100 ns since 1601, UTC), 0 when unset.
    pub modified: u64,
    /// Creation time as a Windows FILETIME (UTC), 0 when unset (streams leave it unset).
    pub created: u64,
}

/// A parsed compound file.
#[derive(Debug, Clone)]
pub struct Cfb {
    /// Major version (3: 512-byte sectors, 4: 4096-byte sectors).
    pub version: u16,
    /// Sector size in bytes (512 or 4096).
    pub sector_size: u64,
    /// Mini-sector size in bytes (64): small streams are stored in these.
    pub mini_sector_size: u64,
    /// Streams smaller than this many bytes live in the mini stream (usually 4096).
    pub mini_cutoff: u64,
    /// Every directory entry, in directory order.
    pub entries: Vec<CfbEntry>,
    fat: Vec<u32>,
    minifat: Vec<u32>,
    /// Sectors of the mini stream (the root entry's stream).
    ministream: Vec<u32>,
    file_len: u64,
    /// Problems found while parsing (unreadable sectors, loops, sizes past the end).
    pub problems: Vec<String>,
    /// Format id used in errors.
    format: &'static str,
}

fn sector_offset(sector_size: u64, s: u32) -> u64 {
    (u64::from(s) + 1) * sector_size
}

impl Cfb {
    /// Parse the structure (header, FATs, directory) of an open file.
    pub fn open<R: Read + Seek + ?Sized>(
        f: &mut R,
        path: &Path,
        format: &'static str,
    ) -> Result<Cfb> {
        let file_len = f.seek(SeekFrom::End(0)).map_err(|e| Error::io(path, e))?;
        let h = read_range(f, path, 0, 512)?;
        if h.len() < 512 || h[..8] != CFB_MAGIC {
            return Err(Error::corrupt(
                format,
                "not a compound file (no D0 CF 11 E0 signature)",
            ));
        }
        let version = le_u16(&h, 0x1A).unwrap_or(0);
        let shift = le_u16(&h, 0x1E).unwrap_or(0);
        let mini_shift = le_u16(&h, 0x20).unwrap_or(0);
        if !(shift == 9 || shift == 12) || mini_shift != 6 {
            return Err(Error::corrupt(
                format,
                format!(
                    "compound file sector shift {shift} / mini sector shift {mini_shift} (expected 9 or 12 / 6)"
                ),
            ));
        }
        let sector_size = 1u64 << shift;
        let n_fat = le_u32(&h, 0x2C).unwrap_or(0) as usize;
        let first_dir = le_u32(&h, 0x30).unwrap_or(END_OF_CHAIN);
        let mini_cutoff = u64::from(le_u32(&h, 0x38).unwrap_or(4096));
        let first_minifat = le_u32(&h, 0x3C).unwrap_or(END_OF_CHAIN);
        let n_minifat = le_u32(&h, 0x40).unwrap_or(0) as usize;
        let mut difat_next = le_u32(&h, 0x44).unwrap_or(END_OF_CHAIN);
        let n_difat = le_u32(&h, 0x48).unwrap_or(0) as usize;
        let mut problems = Vec::new();
        let max_sectors = file_len / sector_size + 1;
        if n_fat as u64 > max_sectors {
            return Err(Error::corrupt(
                format,
                format!("{n_fat} FAT sectors in a {file_len}-byte file"),
            ));
        }
        // DIFAT: 109 entries in the header, then chained DIFAT sectors
        let mut fat_sectors: Vec<u32> = (0..109)
            .filter_map(|i| le_u32(&h, 0x4C + 4 * i))
            .filter(|&s| s != FREE)
            .collect();
        let per = (sector_size / 4) as usize;
        let mut seen = BTreeSet::new();
        for _ in 0..n_difat {
            if difat_next == END_OF_CHAIN || difat_next == FREE || !seen.insert(difat_next) {
                break;
            }
            let b = read_range(f, path, sector_offset(sector_size, difat_next), sector_size)?;
            for i in 0..per - 1 {
                if let Some(s) = le_u32(&b, 4 * i)
                    && s != FREE
                {
                    fat_sectors.push(s);
                }
            }
            difat_next = le_u32(&b, 4 * (per - 1)).unwrap_or(END_OF_CHAIN);
        }
        fat_sectors.truncate(n_fat);
        let mut fat = Vec::with_capacity(n_fat * per);
        for &s in &fat_sectors {
            let off = sector_offset(sector_size, s);
            if off + sector_size > file_len {
                problems.push(format!("FAT sector {s} lies past the end of the file"));
                fat.extend(std::iter::repeat_n(FREE, per));
                continue;
            }
            let b = read_range(f, path, off, sector_size)?;
            fat.extend((0..per).filter_map(|i| le_u32(&b, 4 * i)));
        }
        let mut cfb = Cfb {
            version,
            sector_size,
            mini_sector_size: 1 << mini_shift,
            mini_cutoff,
            entries: Vec::new(),
            fat,
            minifat: Vec::new(),
            ministream: Vec::new(),
            file_len,
            problems,
            format,
        };
        // directory
        let dir_sectors = cfb.chain(first_dir)?;
        let mut dir = Vec::new();
        for s in &dir_sectors {
            dir.extend(read_range(
                f,
                path,
                sector_offset(sector_size, *s),
                sector_size,
            )?);
        }
        // mini FAT
        let mf = cfb.chain(first_minifat)?;
        if mf.len() < n_minifat {
            cfb.problems.push(format!(
                "mini FAT chain has {} sectors, header says {n_minifat}",
                mf.len()
            ));
        }
        for s in mf {
            let b = read_range(f, path, sector_offset(sector_size, s), sector_size)?;
            cfb.minifat
                .extend((0..per).filter_map(|i| le_u32(&b, 4 * i)));
        }
        cfb.entries = cfb.walk_directory(&dir);
        if let Some(root) = dir.get(..128) {
            let start = le_u32(root, 0x74).unwrap_or(END_OF_CHAIN);
            cfb.ministream = cfb.chain(start)?;
        }
        Ok(cfb)
    }

    /// Follow a FAT chain from `start`.
    fn chain(&self, start: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut s = start;
        let mut seen = BTreeSet::new();
        while s != END_OF_CHAIN && s != FREE {
            if !seen.insert(s) || out.len() > MAX_CHAIN {
                return Err(Error::corrupt(
                    self.format,
                    format!("compound-file sector chain loops at sector {s}"),
                ));
            }
            out.push(s);
            s = *self.fat.get(s as usize).ok_or_else(|| {
                Error::corrupt(
                    self.format,
                    format!("sector {s} is beyond the FAT (truncated or corrupt)"),
                )
            })?;
        }
        Ok(out)
    }

    fn mini_chain(&self, start: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut s = start;
        let mut seen = BTreeSet::new();
        while s != END_OF_CHAIN && s != FREE {
            if !seen.insert(s) || out.len() > MAX_CHAIN {
                return Err(Error::corrupt(self.format, "mini-stream chain loops"));
            }
            out.push(s);
            s = *self
                .minifat
                .get(s as usize)
                .ok_or_else(|| Error::corrupt(self.format, "mini sector beyond the mini FAT"))?;
        }
        Ok(out)
    }

    fn walk_directory(&mut self, dir: &[u8]) -> Vec<CfbEntry> {
        let n = dir.len() / 128;
        let rec = |i: u32| dir.get(i as usize * 128..i as usize * 128 + 128);
        let mut out = Vec::new();
        // (entry id, parent path) stack; siblings form a binary tree under each storage's child
        let mut stack: Vec<(u32, String)> = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(root) = rec(0) {
            let child = le_u32(root, 0x4C).unwrap_or(NO_STREAM);
            out.push(CfbEntry {
                id: 0,
                path: String::new(),
                is_stream: false,
                size: 0,
                start_sector: le_u32(root, 0x74).unwrap_or(END_OF_CHAIN),
                modified: 0,
                created: 0,
            });
            seen.insert(0);
            if child != NO_STREAM {
                stack.push((child, String::new()));
            }
        }
        while let Some((id, parent)) = stack.pop() {
            if id as usize >= n || !seen.insert(id) {
                if id as usize >= n {
                    self.problems
                        .push(format!("directory entry {id} out of range"));
                } else {
                    self.problems
                        .push(format!("directory entry {id} reached twice (loop)"));
                }
                continue;
            }
            let Some(r) = rec(id) else { continue };
            let name_len = le_u16(r, 0x40).unwrap_or(0) as usize;
            let name = utf16le_z(&r[..name_len.min(64)]);
            let kind = r[0x42];
            for off in [0x44, 0x48] {
                let sib = le_u32(r, off).unwrap_or(NO_STREAM);
                if sib != NO_STREAM {
                    stack.push((sib, parent.clone()));
                }
            }
            let path = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            let size_lo = u64::from(le_u32(r, 0x78).unwrap_or(0));
            let size_hi = u64::from(le_u32(r, 0x7C).unwrap_or(0));
            let size = if self.version == 3 {
                size_lo
            } else {
                size_hi << 32 | size_lo
            };
            let modified = u64::from(le_u32(r, 0x6C).unwrap_or(0))
                | u64::from(le_u32(r, 0x70).unwrap_or(0)) << 32;
            let created = u64::from(le_u32(r, 0x64).unwrap_or(0))
                | u64::from(le_u32(r, 0x68).unwrap_or(0)) << 32;
            match kind {
                1 => {
                    let child = le_u32(r, 0x4C).unwrap_or(NO_STREAM);
                    if child != NO_STREAM {
                        stack.push((child, path.clone()));
                    }
                    out.push(CfbEntry {
                        id,
                        path,
                        is_stream: false,
                        size: 0,
                        start_sector: 0,
                        modified,
                        created,
                    });
                }
                2 => out.push(CfbEntry {
                    id,
                    path,
                    is_stream: true,
                    size,
                    start_sector: le_u32(r, 0x74).unwrap_or(END_OF_CHAIN),
                    modified,
                    created,
                }),
                _ => self
                    .problems
                    .push(format!("directory entry {id} ({name}) has type {kind}")),
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Find a stream by path (case-insensitive).
    pub fn stream(&self, path: &str) -> Option<&CfbEntry> {
        self.entries
            .iter()
            .find(|e| e.is_stream && e.path.eq_ignore_ascii_case(path))
    }

    /// The sectors of a stream stored in regular sectors (at or above the mini cutoff), in
    /// order; `None` for a mini stream. For reading ranges of a large stream
    /// ([`Cfb::read_range`]) without reading it whole.
    pub fn sectors(&self, e: &CfbEntry) -> Result<Option<Vec<u32>>> {
        if e.size < self.mini_cutoff {
            return Ok(None);
        }
        let chain = self.chain(e.start_sector)?;
        let needed = e.size.div_ceil(self.sector_size);
        if (chain.len() as u64) < needed {
            return Err(Error::corrupt(
                self.format,
                format!(
                    "stream {}: {} of {needed} sectors reachable through its chain",
                    e.path,
                    chain.len()
                ),
            ));
        }
        Ok(Some(chain))
    }

    /// Read `len` bytes at `offset` of a stream of `size` bytes stored in `sectors`
    /// ([`Cfb::sectors`]). A range past the stream's end is an error.
    pub fn read_range<R: Read + Seek + ?Sized>(
        &self,
        f: &mut R,
        path: &Path,
        sectors: &[u32],
        size: u64,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|e| *e <= size)
            .ok_or_else(|| {
                Error::corrupt_at(
                    self.format,
                    offset,
                    format!("{len} bytes at {offset} run past the end of a {size}-byte stream"),
                )
            })?;
        let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0).min(1 << 26));
        let mut pos = offset;
        while pos < end {
            let k = usize::try_from(pos / self.sector_size).unwrap_or(usize::MAX);
            let within = pos % self.sector_size;
            let s = *sectors.get(k).ok_or_else(|| {
                Error::corrupt(self.format, "stream range beyond its sector chain")
            })?;
            let want = (self.sector_size - within).min(end - pos);
            let off = sector_offset(self.sector_size, s) + within;
            if off + want > self.file_len {
                return Err(Error::corrupt_at(
                    self.format,
                    off,
                    "stream data past the end of the file (truncated)",
                ));
            }
            out.extend(read_range(f, path, off, want)?);
            pos += want;
        }
        Ok(out)
    }

    /// Read a whole stream (at most `limit` bytes).
    pub fn read<R: Read + Seek + ?Sized>(
        &self,
        f: &mut R,
        path: &Path,
        e: &CfbEntry,
        limit: u64,
    ) -> Result<Vec<u8>> {
        let size = e.size.min(limit);
        let mut out = Vec::with_capacity(usize::try_from(size).unwrap_or(0).min(1 << 26));
        if e.size < self.mini_cutoff {
            let ms = self.mini_sector_size;
            let per = self.sector_size / ms;
            for m in self.mini_chain(e.start_sector)? {
                if out.len() as u64 >= size {
                    break;
                }
                let big = u64::from(m) / per;
                let within = u64::from(m) % per;
                let sec = *self.ministream.get(big as usize).ok_or_else(|| {
                    Error::corrupt(
                        self.format,
                        format!("stream {}: mini sector {m} outside the mini stream", e.path),
                    )
                })?;
                let off = sector_offset(self.sector_size, sec) + within * ms;
                let want = ms.min(size - out.len() as u64);
                out.extend(read_range(f, path, off, want)?);
            }
        } else {
            for s in self.chain(e.start_sector)? {
                if out.len() as u64 >= size {
                    break;
                }
                let want = self.sector_size.min(size - out.len() as u64);
                let off = sector_offset(self.sector_size, s);
                if off + want > self.file_len {
                    return Err(Error::corrupt_at(
                        self.format,
                        off,
                        format!(
                            "stream {} runs past the end of the file (truncated)",
                            e.path
                        ),
                    ));
                }
                out.extend(read_range(f, path, off, want)?);
            }
        }
        if (out.len() as u64) < size {
            return Err(Error::corrupt(
                self.format,
                format!(
                    "stream {}: {} of {} bytes reachable through its sector chain",
                    e.path,
                    out.len(),
                    e.size
                ),
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal version-3 compound file: header, FAT sector 0, directory sector 1 with a root
    /// and one 600-byte regular stream "Data" in sectors 2–3.
    pub(crate) fn sample() -> Vec<u8> {
        let mut f = vec![0u8; 512 * 5];
        f[..8].copy_from_slice(&CFB_MAGIC);
        f[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
        f[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
        f[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
        f[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
        f[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
        f[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
        f[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
        // a 512-byte mini-stream cutoff keeps the 600-byte stream in regular sectors
        f[0x38..0x3C].copy_from_slice(&512u32.to_le_bytes());
        f[0x3C..0x40].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        f[0x44..0x48].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        for i in 0..109 {
            f[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&FREE.to_le_bytes());
        }
        f[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
        // FAT at sector 0 (file offset 512)
        let fat = [0xFFFF_FFFDu32, END_OF_CHAIN, 3, END_OF_CHAIN];
        for (i, v) in fat.iter().enumerate() {
            f[512 + 4 * i..516 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        for i in 4..128 {
            f[512 + 4 * i..516 + 4 * i].copy_from_slice(&FREE.to_le_bytes());
        }
        // directory at sector 1 (offset 1024)
        let entry =
            |f: &mut Vec<u8>, i: usize, name: &str, kind: u8, child: u32, start: u32, size: u32| {
                let o = 1024 + 128 * i;
                for (k, u) in name.encode_utf16().enumerate() {
                    f[o + 2 * k..o + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
                }
                f[o + 0x40..o + 0x42].copy_from_slice(&((name.len() as u16 + 1) * 2).to_le_bytes());
                f[o + 0x42] = kind;
                f[o + 0x44..o + 0x48].copy_from_slice(&NO_STREAM.to_le_bytes());
                f[o + 0x48..o + 0x4C].copy_from_slice(&NO_STREAM.to_le_bytes());
                f[o + 0x4C..o + 0x50].copy_from_slice(&child.to_le_bytes());
                f[o + 0x74..o + 0x78].copy_from_slice(&start.to_le_bytes());
                f[o + 0x78..o + 0x7C].copy_from_slice(&size.to_le_bytes());
            };
        entry(&mut f, 0, "Root Entry", 5, 1, END_OF_CHAIN, 0);
        entry(&mut f, 1, "Data", 2, NO_STREAM, 2, 600);
        for (k, b) in f[1536..1536 + 600].iter_mut().enumerate() {
            *b = (k % 251) as u8;
        }
        f
    }

    #[test]
    fn reads_a_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.lcd");
        std::fs::write(&path, sample()).unwrap();
        let mut file = crate::source::SourceFile::open_local(&path).unwrap();
        let cfb = Cfb::open(&mut file, &path, "t").unwrap();
        let entry = cfb.stream("data").unwrap().clone();
        assert_eq!(entry.size, 600);
        let whole = cfb.read(&mut file, &path, &entry, u64::MAX).unwrap();
        assert_eq!(whole.len(), 600);
        assert_eq!(whole[599], (599 % 251) as u8);
        // a range across the sector boundary, without reading the stream whole
        let sectors = cfb.sectors(&entry).unwrap().expect("a regular stream");
        let range = cfb
            .read_range(&mut file, &path, &sectors, entry.size, 500, 60)
            .unwrap();
        assert_eq!(range, whole[500..560].to_vec());
        assert!(
            cfb.read_range(&mut file, &path, &sectors, entry.size, 590, 20)
                .is_err()
        );
        // truncated copy: the stream's second sector is gone
        std::fs::write(&path, &sample()[..2100]).unwrap();
        let mut file = crate::source::SourceFile::open_local(&path).unwrap();
        let cfb = Cfb::open(&mut file, &path, "t").unwrap();
        assert!(cfb.read(&mut file, &path, &entry, u64::MAX).is_err());
    }
}
