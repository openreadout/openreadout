//! The binary container: file header, block index, typed blocks, plane chunk tags and the
//! length-prefixed XML documents inside metadata blocks. Layout: `docs/formats/oir.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{find, le_u32, le_u64};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// First 16 bytes of every OIR file (main and continuation files).
pub const MAGIC: &[u8; 16] = b"OLYMPUSRAWFORMAT";
/// Byte offset of the first block (the header is 96 bytes).
pub const FIRST_BLOCK_OFFSET: u64 = 0x60;
/// First word of the block index.
pub const INDEX_MARKER: u32 = 0xFFFF_FFFF;

/// Largest XML document or metadata block payload we load into memory (256 MiB).
const MAX_PAYLOAD: u32 = 256 << 20;

/// The 96-byte file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OirHeader {
    /// Four words after the magic (`12, 0, 1, 2` in every corpus file).
    pub header_words: [u32; 4],
    /// File size as recorded by the writer.
    pub declared_size: u64,
    /// Offset of the block index (`0xFFFFFFFF` + u64 block offsets).
    pub index_offset: u64,
    /// Number of blocks listed in the index.
    pub block_count: u32,
    /// Offset of the bitmap thumbnail block, if any.
    pub thumbnail_offset: Option<u64>,
    /// Writer tag at 0x48 (`FLUOVIEW`).
    pub producer: String,
}

/// What a block holds (the `kind` word of its 8-byte header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// 0: XML documents (file info, image properties, LUTs, channel settings, ...).
    Documents,
    /// 1: one frame's properties document.
    FrameProperties,
    /// 2: `BMP ` + a Windows bitmap thumbnail.
    Thumbnail,
    /// 3: names the pixel block that follows (plane, channel, chunk; offset in the plane).
    ChunkTag,
    /// 4: raw samples of one chunk of one plane.
    Pixels,
    /// 5: empty block written after each frame.
    Separator,
    /// Any other kind word.
    Unknown(u32),
}

impl BlockKind {
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => BlockKind::Documents,
            1 => BlockKind::FrameProperties,
            2 => BlockKind::Thumbnail,
            3 => BlockKind::ChunkTag,
            4 => BlockKind::Pixels,
            5 => BlockKind::Separator,
            other => BlockKind::Unknown(other),
        }
    }

    pub fn code(self) -> u32 {
        match self {
            BlockKind::Documents => 0,
            BlockKind::FrameProperties => 1,
            BlockKind::Thumbnail => 2,
            BlockKind::ChunkTag => 3,
            BlockKind::Pixels => 4,
            BlockKind::Separator => 5,
            BlockKind::Unknown(c) => c,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BlockKind::Documents => "documents",
            BlockKind::FrameProperties => "frame-properties",
            BlockKind::Thumbnail => "thumbnail",
            BlockKind::ChunkTag => "chunk-tag",
            BlockKind::Pixels => "pixels",
            BlockKind::Separator => "separator",
            BlockKind::Unknown(_) => "unknown",
        }
    }
}

/// One block: `u32 payload_len, u32 kind, payload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub offset: u64,
    pub kind: BlockKind,
    pub payload_len: u32,
}

impl Block {
    pub fn payload_offset(&self) -> u64 {
        self.offset + 8
    }
    pub fn end(&self) -> u64 {
        self.offset + 8 + u64::from(self.payload_len)
    }
}

/// Payload of a chunk-tag block: which plane chunk the next pixel block holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkTag {
    /// e.g. `l001z001_0_1_<uuid>_0` or `REF_LSM0_<uuid>_1`.
    pub name: String,
    /// Byte offset of the chunk inside its plane.
    pub plane_offset: u32,
    /// Byte length of the chunk (equals the pixel block's payload length).
    pub chunk_len: u32,
}

/// A chunk name split into its parts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkName {
    /// 1-based time index (`t003`), 0 when absent.
    pub t: u32,
    /// 1-based lambda index (`l002`), 0 when absent.
    pub lambda: u32,
    /// 1-based z index (`z017`), 0 when absent.
    pub z: u32,
    /// Letters we do not know, with their numbers (e.g. `r002`), empty normally.
    pub unknown_axes: String,
    /// Channel uuid.
    pub channel: String,
    /// `REF_<source>` chunks belong to the reference image; `source` is e.g. `LSM0`.
    pub reference: Option<String>,
    /// Chunk number within the plane.
    pub chunk: u32,
}

impl ChunkName {
    /// Parse `[l001][z001][t001]_<a>_<b>_<uuid>_<chunk>` or `REF_<source>_<uuid>_<chunk>`.
    pub fn parse(name: &str) -> Option<ChunkName> {
        let (rest, chunk) = name.rsplit_once('_')?;
        let chunk: u32 = chunk.parse().ok()?;
        let (head, channel) = rest.rsplit_once('_')?;
        if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return None;
        }
        let mut out = ChunkName {
            t: 0,
            lambda: 0,
            z: 0,
            unknown_axes: String::new(),
            channel: channel.to_string(),
            reference: None,
            chunk,
        };
        if let Some(src) = head.strip_prefix("REF_") {
            out.reference = Some(src.to_string());
            return Some(out);
        }
        // Axis part: before the first '_' (may be empty); the rest is the `_a_b` pair.
        let axes = head.split('_').next().unwrap_or("");
        let bytes = axes.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let letter = bytes[i];
            if !letter.is_ascii_alphabetic() {
                return None;
            }
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == start {
                return None;
            }
            let value: u32 = axes[start..j].parse().ok()?;
            match letter {
                b'l' => out.lambda = value,
                b'z' => out.z = value,
                b't' => out.t = value,
                _ => out.unknown_axes.push_str(&axes[i..j]),
            }
            i = j;
        }
        Some(out)
    }
}

/// An XML document found in a documents or frame-properties block.
#[derive(Debug, Clone)]
pub struct XmlDoc {
    /// Root element name with its prefix, e.g. `lsmimage:imageProperties`.
    pub root: String,
    /// Absolute file offset of the document text.
    pub offset: u64,
    pub text: String,
    /// For `lut:LUT` documents: the channel uuid written right before the document.
    pub channel: Option<String>,
}

/// Extract every length-prefixed XML document of a block payload.
///
/// Each document is preceded by its byte length as a u32; LUT documents are also preceded by
/// `u32 36` and the ASCII uuid of their channel.
pub fn documents(payload: &[u8], payload_offset: u64) -> Vec<XmlDoc> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(found) = find(&payload[from..], b"<?xml") {
        let s = from + found;
        from = s + 5;
        if s < 4 {
            continue;
        }
        let len = le_u32(payload, s - 4).unwrap_or(0) as usize;
        let Some(end) = s.checked_add(len).filter(|e| *e <= payload.len()) else {
            continue;
        };
        let raw = &payload[s..end];
        let text = String::from_utf8_lossy(raw).into_owned();
        if !text.trim_end().ends_with('>') {
            continue;
        }
        let root = root_name(&text).unwrap_or_default();
        let channel = (s >= 44 && le_u32(payload, s - 44) == Some(36))
            .then(|| String::from_utf8_lossy(&payload[s - 40..s - 4]).into_owned())
            .filter(|u| u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        out.push(XmlDoc {
            root,
            offset: payload_offset + s as u64,
            text,
            channel,
        });
        from = end;
    }
    out
}

/// Name of the first element after the XML declaration.
fn root_name(text: &str) -> Option<String> {
    let mut rest = text;
    loop {
        let i = rest.find('<')?;
        rest = &rest[i + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
            .collect();
        return Some(name);
    }
}

/// Random-access reader with a 1 MiB read-ahead window (block headers are read in file order).
#[derive(Debug)]
pub(crate) struct WindowReader {
    file: SourceFile,
    path: PathBuf,
    pub len: u64,
    win_start: u64,
    win: Vec<u8>,
}

const WINDOW: usize = 1 << 20;

impl WindowReader {
    pub fn open(fs: &Fs, path: &Path) -> Result<Self> {
        let file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        Ok(WindowReader {
            file,
            path: path.to_path_buf(),
            len,
            win_start: 0,
            win: Vec::new(),
        })
    }

    /// Exactly `n` bytes at `offset`, or a corrupt-file error naming what was being read.
    pub fn read_at(&mut self, offset: u64, n: usize, what: &str) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(n as u64)
            .filter(|e| *e <= self.len)
            .ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    offset,
                    format!("{what} runs past the end of the file (file truncated?)"),
                )
            })?;
        if n <= WINDOW / 4 {
            let win_end = self.win_start + self.win.len() as u64;
            if offset < self.win_start || end > win_end {
                let want = WINDOW.min(usize::try_from(self.len - offset).unwrap_or(WINDOW));
                self.win = vec![0u8; want];
                self.fill(offset)?;
                self.win_start = offset;
            }
            let s = usize::try_from(offset - self.win_start).unwrap_or(0);
            return Ok(self.win[s..s + n].to_vec());
        }
        let mut buf = vec![0u8; n];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&self.path, e))?;
        self.file
            .read_exact(&mut buf)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(buf)
    }

    fn fill(&mut self, offset: u64) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&self.path, e))?;
        self.file
            .read_exact(&mut self.win)
            .map_err(|e| Error::io(&self.path, e))
    }

    /// Read into `buf` at `offset` without the window (pixel data).
    pub fn read_into(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|e| e > self.len)
        {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                offset,
                "pixel block runs past the end of the file (file truncated?)",
            ));
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&self.path, e))?;
        self.file
            .read_exact(buf)
            .map_err(|e| Error::io(&self.path, e))
    }
}

/// Does `head` start with the OIR signature?
pub fn looks_like_oir(head: &[u8]) -> bool {
    head.len() >= 16 && &head[..16] == MAGIC
}

/// A parsed OIR container (a main file or a continuation file).
#[derive(Debug)]
pub struct OirFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub header: OirHeader,
    /// Blocks in file order.
    pub blocks: Vec<Block>,
    /// Chunk tags, keyed by the index of their tag block in `blocks`.
    pub chunk_tags: Vec<(usize, ChunkTag)>,
    /// Why the index could not be used (the blocks were then found by walking the chain).
    pub index_problem: Option<String>,
    /// Offset where the block chain runs past the end of the file.
    pub truncated_at: Option<u64>,
    /// Offset and reason where the block chain stopped making sense.
    pub bad_block_at: Option<(u64, String)>,
    pub(crate) reader: WindowReader,
}

impl OirFile {
    /// Open and index a file; a damaged index falls back to walking the block chain.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open and index `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut reader = WindowReader::open(fs, path)?;
        let file_len = reader.len;
        if file_len < FIRST_BLOCK_OFFSET {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("file is {file_len} bytes, shorter than the 96-byte OIR header"),
            ));
        }
        let h = reader.read_at(0, FIRST_BLOCK_OFFSET as usize, "file header")?;
        if !looks_like_oir(&h) {
            return Err(Error::corrupt(
                FORMAT_ID,
                "file does not start with OLYMPUSRAWFORMAT",
            ));
        }
        let thumb = le_u64(&h, 0x40).unwrap_or(0);
        let header = OirHeader {
            header_words: [
                le_u32(&h, 16).unwrap_or(0),
                le_u32(&h, 20).unwrap_or(0),
                le_u32(&h, 24).unwrap_or(0),
                le_u32(&h, 28).unwrap_or(0),
            ],
            declared_size: le_u64(&h, 0x20).unwrap_or(0),
            index_offset: le_u64(&h, 0x28).unwrap_or(0),
            block_count: le_u32(&h, 0x30).unwrap_or(0),
            thumbnail_offset: (thumb != u64::MAX && thumb != 0).then_some(thumb),
            producer: String::from_utf8_lossy(&h[0x48..0x50])
                .trim_end_matches('\0')
                .to_string(),
        };
        let mut f = OirFile {
            path: path.to_path_buf(),
            file_len,
            header,
            blocks: Vec::new(),
            chunk_tags: Vec::new(),
            index_problem: None,
            truncated_at: None,
            bad_block_at: None,
            reader,
        };
        match f.index_offsets() {
            Ok(offsets) => f.load_blocks(&offsets)?,
            Err(why) => {
                f.index_problem = Some(why);
                f.walk_chain()?;
            }
        }
        f.load_chunk_tags()?;
        Ok(f)
    }

    /// Block offsets from the index, or why the index is unusable.
    fn index_offsets(&mut self) -> std::result::Result<Vec<u64>, String> {
        let h = &self.header;
        // Stale bytes may follow the recorded end (seen: 237 bytes of an older, longer index);
        // a recorded size beyond the end of the file means the file is truncated.
        if h.declared_size > self.file_len {
            return Err(format!(
                "header records a file size of {} bytes but the file is {} bytes",
                h.declared_size, self.file_len
            ));
        }
        if h.index_offset < FIRST_BLOCK_OFFSET || h.index_offset.saturating_add(4) > h.declared_size
        {
            return Err(format!(
                "index offset {} is outside the recorded file",
                h.index_offset
            ));
        }
        let n = usize::try_from(h.declared_size - h.index_offset)
            .map_err(|_| "index too large".to_string())?;
        if n > 64 << 20 {
            return Err(format!("index of {n} bytes is implausibly large"));
        }
        let idx = self
            .reader
            .read_at(h.index_offset, n, "block index")
            .map_err(|e| e.to_string())?;
        if le_u32(&idx, 0) != Some(INDEX_MARKER) {
            return Err("index does not start with 0xFFFFFFFF".into());
        }
        let mut out = Vec::new();
        let mut i = 4;
        while i + 8 <= idx.len() {
            let o = le_u64(&idx, i).unwrap_or(0);
            if o == 0 || o >= h.index_offset {
                break;
            }
            out.push(o);
            i += 8;
        }
        Ok(out)
    }

    fn load_blocks(&mut self, offsets: &[u64]) -> Result<()> {
        let mut sorted = offsets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for o in sorted {
            if o + 8 > self.file_len {
                self.bad_block_at = Some((o, "index points past the end of the file".into()));
                continue;
            }
            let b = self.reader.read_at(o, 8, "block header")?;
            self.blocks.push(Block {
                offset: o,
                kind: BlockKind::from_code(le_u32(&b, 4).unwrap_or(0)),
                payload_len: le_u32(&b, 0).unwrap_or(0),
            });
        }
        Ok(())
    }

    /// Recover blocks by following `payload_len` from the first block (index missing or damaged).
    fn walk_chain(&mut self) -> Result<()> {
        let mut o = FIRST_BLOCK_OFFSET;
        let stop = if self.header.index_offset > FIRST_BLOCK_OFFSET
            && self.header.index_offset <= self.file_len
        {
            self.header.index_offset
        } else {
            self.file_len
        };
        while o + 8 <= stop {
            let b = self.reader.read_at(o, 8, "block header")?;
            let len = le_u32(&b, 0).unwrap_or(0);
            let kind = le_u32(&b, 4).unwrap_or(0);
            if kind > 64 {
                self.bad_block_at = Some((o, format!("block kind {kind} is not a known kind")));
                return Ok(());
            }
            let block = Block {
                offset: o,
                kind: BlockKind::from_code(kind),
                payload_len: len,
            };
            if block.end() > self.file_len {
                self.truncated_at = Some(o);
                self.blocks.push(block);
                return Ok(());
            }
            self.blocks.push(block);
            o = block.end();
        }
        if o < stop && stop == self.file_len {
            self.truncated_at = Some(o);
        }
        Ok(())
    }

    fn load_chunk_tags(&mut self) -> Result<()> {
        for i in 0..self.blocks.len() {
            let b = self.blocks[i];
            if b.kind != BlockKind::ChunkTag {
                continue;
            }
            if b.payload_len < 12 || b.end() > self.file_len {
                continue;
            }
            let p = self.reader.read_at(
                b.payload_offset(),
                (b.payload_len as usize).min(4096),
                "chunk tag",
            )?;
            let name_len = le_u32(&p, 8).unwrap_or(0) as usize;
            if 12 + name_len > p.len() {
                continue;
            }
            self.chunk_tags.push((
                i,
                ChunkTag {
                    name: String::from_utf8_lossy(&p[12..12 + name_len]).into_owned(),
                    plane_offset: le_u32(&p, 0).unwrap_or(0),
                    chunk_len: le_u32(&p, 4).unwrap_or(0),
                },
            ));
        }
        Ok(())
    }

    /// The payload of block `i`, loaded whole (documents, frame properties, thumbnail).
    pub fn payload(&mut self, i: usize) -> Result<Vec<u8>> {
        let b = self.blocks[i];
        if b.payload_len > MAX_PAYLOAD {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                b.offset,
                format!(
                    "{} block of {} bytes is implausibly large",
                    b.kind.name(),
                    b.payload_len
                ),
            ));
        }
        self.reader
            .read_at(b.payload_offset(), b.payload_len as usize, b.kind.name())
    }

    /// The pixel block following chunk tag `tag_block` (the next block in file order).
    pub fn pixels_after(&self, tag_block: usize) -> Option<Block> {
        self.blocks
            .get(tag_block + 1)
            .copied()
            .filter(|b| b.kind == BlockKind::Pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_names() {
        let n = ChunkName::parse("l001z002_0_1_93e4632f-0342-4d98-bdc1-4ce305b92525_46").unwrap();
        assert_eq!((n.lambda, n.z, n.t, n.chunk), (1, 2, 0, 46));
        assert_eq!(n.channel, "93e4632f-0342-4d98-bdc1-4ce305b92525");
        assert!(n.reference.is_none());
        let n = ChunkName::parse("t003z002_0_1_d59928c9-7471-4e5b-a88f-f850be707c65_1").unwrap();
        assert_eq!((n.t, n.z), (3, 2));
        let n = ChunkName::parse("REF_LSM0_d59928c9-7471-4e5b-a88f-f850be707c65_0").unwrap();
        assert_eq!(n.reference.as_deref(), Some("LSM0"));
        let n = ChunkName::parse("_0_1_d59928c9-7471-4e5b-a88f-f850be707c65_0").unwrap();
        assert_eq!((n.t, n.z, n.lambda), (0, 0, 0));
        let n = ChunkName::parse("r002z001_0_1_d59928c9_0").unwrap();
        assert_eq!(n.unknown_axes, "r002");
        assert!(ChunkName::parse("garbage").is_none());
        assert!(ChunkName::parse("z01x_0_1_abc_q").is_none());
    }

    #[test]
    fn extracts_length_prefixed_documents() {
        let a = b"<?xml version=\"1.0\"?>\r\n<a:b x=\"1\"/>";
        let lut = b"<?xml version=\"1.0\"?><lut:LUT/>";
        let uuid = b"93e4632f-0342-4d98-bdc1-4ce305b92525";
        let mut p = vec![7u8; 36];
        p.extend_from_slice(&(a.len() as u32).to_le_bytes());
        p.extend_from_slice(a);
        p.extend_from_slice(&[1, 2, 3]);
        p.extend_from_slice(&36u32.to_le_bytes());
        p.extend_from_slice(uuid);
        p.extend_from_slice(&(lut.len() as u32).to_le_bytes());
        p.extend_from_slice(lut);
        let docs = documents(&p, 1000);
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].root, "a:b");
        assert_eq!(docs[0].offset, 1040);
        assert!(docs[0].channel.is_none());
        assert_eq!(docs[1].root, "lut:LUT");
        assert_eq!(
            docs[1].channel.as_deref(),
            Some(std::str::from_utf8(uuid).unwrap())
        );
        // a wrong length prefix is not a document
        let mut bad = 999u32.to_le_bytes().to_vec();
        bad.extend_from_slice(a);
        assert!(documents(&bad, 0).is_empty());
    }

    #[test]
    fn block_kinds_round_trip() {
        for c in 0..8 {
            assert_eq!(BlockKind::from_code(c).code(), c);
        }
        assert_eq!(BlockKind::from_code(9), BlockKind::Unknown(9));
    }
}
