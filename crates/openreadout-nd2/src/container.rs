//! Chunked container: chunk headers, the chunk map, and the rescue scan. See `docs/formats/nd2.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{le_u32, le_u64};
use openreadout_core::limits::{checked_len, checked_metadata_len, remaining};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Chunk magic as a u32 (little-endian in the file).
pub const CHUNK_MAGIC: u32 = 0x0ABE_CEDA;
/// The same magic as it appears on disk.
pub const CHUNK_MAGIC_BYTES: [u8; 4] = [0xDA, 0xCE, 0xBE, 0x0A];
/// Name of the first chunk.
pub const FILE_SIGNATURE: &str = "ND2 FILE SIGNATURE CHUNK NAME01!";
/// Name of the chunk-map chunk.
pub const FILE_MAP_NAME: &str = "ND2 FILEMAP SIGNATURE NAME 0001!";
/// Terminator entry inside the chunk map (also written just before the trailing offset).
pub const CHUNK_MAP_SIGNATURE: &str = "ND2 CHUNK MAP SIGNATURE 0000001!";
/// JPEG 2000 signature box that legacy files start with.
pub const LEGACY_SIGNATURE: [u8; 12] = [
    0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// A chunk header as stored.
#[derive(Debug, Clone)]
pub struct ChunkHeader {
    pub name: String,
    pub name_len: u32,
    pub data_len: u64,
    /// Offset of the payload.
    pub payload_offset: u64,
}

/// One chunk-map entry.
#[derive(Debug, Clone)]
pub struct ChunkMapEntry {
    pub name: String,
    /// Offset of the chunk header.
    pub chunk_offset: u64,
    pub chunk_data_len: u64,
}

/// An opened ND2 container.
#[derive(Debug)]
pub struct Nd2File {
    pub path: PathBuf,
    pub file_len: u64,
    pub version: String,
    pub map: Vec<ChunkMapEntry>,
    pub chunk_map_offset: Option<u64>,
    /// True when the map was rebuilt by scanning.
    pub rescued: bool,
    pub problems: Vec<(Option<u64>, String)>,
}

/// Read `len` bytes at `off`. The length is checked against the file size before the
/// buffer is allocated, so a corrupt size field yields `corrupt_file`, not an OOM abort.
pub(crate) fn read_at(f: &mut SourceFile, path: &Path, off: u64, len: usize) -> Result<Vec<u8>> {
    let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    let len = checked_len(FORMAT_ID, len as u64, remaining(file_len, off), "read").map_err(
        |e| match e {
            Error::Corrupt { format, detail, .. } => Error::Corrupt {
                format,
                detail: format!("{detail} (truncated file)"),
                offset: Some(off),
            },
            other => other,
        },
    )?;
    let mut buf = vec![0u8; len];
    f.seek(SeekFrom::Start(off))
        .map_err(|e| Error::io(path, e))?;
    f.read_exact(&mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::corrupt_at(
                FORMAT_ID,
                off,
                format!("file ends inside a {len}-byte read (truncated file)"),
            )
        } else {
            Error::io(path, e)
        }
    })?;
    Ok(buf)
}

/// Read and validate a chunk header at `off`.
pub(crate) fn read_chunk_header(
    f: &mut SourceFile,
    path: &Path,
    off: u64,
    file_len: u64,
) -> Result<ChunkHeader> {
    let h = read_at(f, path, off, 16)?;
    if le_u32(&h, 0) != Some(CHUNK_MAGIC) {
        return Err(Error::corrupt_at(FORMAT_ID, off, "chunk magic missing"));
    }
    let name_len = le_u32(&h, 4).unwrap_or(0);
    let data_len = le_u64(&h, 8).unwrap_or(0);
    if name_len > 65_536 || off.saturating_add(16 + u64::from(name_len)) > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!("implausible chunk name length {name_len}"),
        ));
    }
    let nb = read_at(f, path, off + 16, name_len as usize)?;
    let name = String::from_utf8_lossy(&nb)
        .trim_end_matches('\0')
        .to_string();
    Ok(ChunkHeader {
        name,
        name_len,
        data_len,
        payload_offset: off + 16 + u64::from(name_len),
    })
}

pub(crate) fn parse_map(payload: &[u8]) -> Vec<ChunkMapEntry> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < payload.len() {
        let Some(bang) = payload[i..].iter().position(|&b| b == b'!') else {
            break;
        };
        let end = i + bang + 1;
        if end + 16 > payload.len() {
            break;
        }
        let name = String::from_utf8_lossy(&payload[i..end]).to_string();
        let chunk_offset = le_u64(payload, end).unwrap_or(0);
        let chunk_data_len = le_u64(payload, end + 8).unwrap_or(0);
        i = end + 16;
        if name == CHUNK_MAP_SIGNATURE {
            break;
        }
        out.push(ChunkMapEntry {
            name,
            chunk_offset,
            chunk_data_len,
        });
    }
    out
}

/// Walk the file looking for chunk headers; used when the chunk map is unusable.
pub(crate) fn rescue_scan(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
) -> Result<Vec<ChunkMapEntry>> {
    let mut out = Vec::new();
    let mut off = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    let mut carry: Vec<u8> = Vec::new();
    let mut base = 0u64;
    while off < file_len {
        f.seek(SeekFrom::Start(off))
            .map_err(|e| Error::io(path, e))?;
        let n = f.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        let mut window = std::mem::take(&mut carry);
        base = off - window.len() as u64;
        window.extend_from_slice(&buf[..n]);
        let mut i = 0usize;
        while i + 16 <= window.len() {
            if window[i..i + 4] == CHUNK_MAGIC_BYTES {
                let name_len = le_u32(&window, i + 4).unwrap_or(0) as usize;
                let data_len = le_u64(&window, i + 8).unwrap_or(0);
                if name_len > 0 && name_len <= 4096 && i + 16 + name_len <= window.len() {
                    let name = String::from_utf8_lossy(&window[i + 16..i + 16 + name_len])
                        .trim_end_matches('\0')
                        .to_string();
                    if name.ends_with('!') && name.is_ascii() && data_len < file_len {
                        out.push(ChunkMapEntry {
                            name,
                            chunk_offset: base + i as u64,
                            chunk_data_len: data_len,
                        });
                        i += 16 + name_len;
                        continue;
                    }
                }
            }
            i += 1;
        }
        let keep = window.len().min(8192);
        carry = window[window.len() - keep..].to_vec();
        off += n as u64;
    }
    let _ = base;
    Ok(out)
}

impl Nd2File {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut problems = Vec::new();
        if file_len < 64 {
            return Err(Error::corrupt(FORMAT_ID, "file too small to be an ND2"));
        }
        let head = read_at(&mut f, path, 0, 12)?;
        if head == LEGACY_SIGNATURE {
            return Err(Error::unsupported(
                FORMAT_ID,
                "legacy JPEG 2000-based ND2 (NIS-Elements 2.x)",
                "Only the chunk-based format (Ver2/Ver3) is readable; re-export from NIS-Elements or use the vendor viewer.",
            ));
        }
        let first = read_chunk_header(&mut f, path, 0, file_len)?;
        if first.name != FILE_SIGNATURE {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                format!("first chunk is '{}' not the file signature", first.name),
            ));
        }
        let vb = read_at(
            &mut f,
            path,
            first.payload_offset,
            first.data_len.min(64) as usize,
        )?;
        let version = String::from_utf8_lossy(&vb)
            .trim_end_matches('\0')
            .trim_start_matches("Ver")
            .to_string();

        // Chunk map from the tail.
        let tail = read_at(&mut f, path, file_len - 8, 8)?;
        let cm_off = le_u64(&tail, 0).unwrap_or(0);
        let mut map = Vec::new();
        let mut rescued = false;
        let mut chunk_map_offset = None;
        if cm_off != 0 && cm_off.saturating_add(16) < file_len {
            match read_chunk_header(&mut f, path, cm_off, file_len) {
                Ok(h)
                    if h.name == FILE_MAP_NAME
                        && h.payload_offset.saturating_add(h.data_len) <= file_len =>
                {
                    let payload = checked_metadata_len(
                        FORMAT_ID,
                        h.data_len,
                        remaining(file_len, h.payload_offset),
                        "chunk map",
                    )
                    .and_then(|len| read_at(&mut f, path, h.payload_offset, len));
                    match payload {
                        Ok(payload) => {
                            map = parse_map(&payload);
                            chunk_map_offset = Some(cm_off);
                        }
                        Err(e) => {
                            problems.push((Some(cm_off), format!("chunk map unreadable: {e}")));
                        }
                    }
                }
                Ok(h) => problems.push((
                    Some(cm_off),
                    format!(
                        "trailing offset points at chunk '{}' instead of the chunk map",
                        h.name
                    ),
                )),
                Err(e) => problems.push((Some(cm_off), format!("chunk map unreadable: {e}"))),
            }
        } else {
            problems.push((
                Some(file_len - 8),
                "trailing chunk-map offset is missing or out of range".into(),
            ));
        }
        let has_attrs = |m: &[ChunkMapEntry]| {
            m.iter()
                .any(|e| e.name == "ImageAttributesLV!" || e.name == "ImageAttributes!")
        };
        if map.is_empty() || !has_attrs(&map) {
            let scanned = rescue_scan(&mut f, path, file_len)?;
            if has_attrs(&scanned) {
                problems.push((None, format!("chunk map unusable; recovered {} chunks by scanning (interrupted acquisition?)", scanned.len())));
                map = scanned;
                rescued = true;
            } else {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    "no chunk map and no recoverable ImageAttributesLV chunk",
                ));
            }
        }
        Ok(Nd2File {
            path: path.to_path_buf(),
            file_len,
            version,
            map,
            chunk_map_offset,
            rescued,
            problems,
        })
    }

    pub fn entry(&self, name: &str) -> Option<&ChunkMapEntry> {
        self.map.iter().find(|e| e.name == name)
    }

    /// Read a chunk's payload by name, validating the header against the map.
    pub fn read_chunk(&self, f: &mut SourceFile, name: &str) -> Result<Vec<u8>> {
        let e = self.entry(name).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("chunk '{name}' not in the chunk map"))
        })?;
        let h = read_chunk_header(f, &self.path, e.chunk_offset, self.file_len)?;
        if h.name != name {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                e.chunk_offset,
                format!("chunk map says '{name}' but the chunk is '{}'", h.name),
            ));
        }
        let len = h.data_len.min(e.chunk_data_len.max(h.data_len));
        if h.payload_offset.saturating_add(len) > self.file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                h.payload_offset,
                format!("chunk '{name}' extends past end of file (truncated)"),
            ));
        }
        let len = checked_metadata_len(
            FORMAT_ID,
            len,
            remaining(self.file_len, h.payload_offset),
            &format!("chunk '{name}'"),
        )?;
        read_at(f, &self.path, h.payload_offset, len)
    }
}
