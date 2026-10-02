//! The binary containers: LIF, LIFEXT and LOF. See `docs/formats/lif.md`.
//!
//! All three start with the same header block (`0x70`, length, `0x2A`, UTF-16 text). A LIF or
//! LIFEXT header holds the XML document and is followed by a chain of memory blocks. A LOF
//! header holds the literal text `LMS_Object_File`; one unnamed payload and then the XML follow.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(crate) use openreadout_core::bytes::utf16le;
use openreadout_core::bytes::{le_u32, le_u64};
use openreadout_core::limits::{checked_metadata_len, remaining};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Block marker at the start of every block (u32 little-endian).
pub const MAGIC: u32 = 0x70;
/// Marker byte that precedes each length-prefixed UTF-16 string and the payload length.
pub const TEXT_MARKER: u8 = 0x2A;
/// Header text of a LOF (single-object) file.
pub const LOF_TEXT: &str = "LMS_Object_File";

/// Which member of the binary family a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// `.lif`: XML header plus a chain of named memory blocks.
    Lif,
    /// `.lifext`: sidecar with `LMSDataContainerEnhancedHeader` XML and named memory blocks.
    Lifext,
    /// `.lof`: one unnamed payload followed by the XML.
    Lof,
}

impl ContainerKind {
    /// Lowercase name used in JSON.
    pub fn name(self) -> &'static str {
        match self {
            ContainerKind::Lif => "lif",
            ContainerKind::Lifext => "lifext",
            ContainerKind::Lof => "lof",
        }
    }
}

/// Parsed file header.
#[derive(Debug, Clone)]
pub struct ContainerHeader {
    /// `block_len` field of the XML block: `2 * xml_len + 5`.
    pub block_len: u32,
    /// UTF-16 code units in the XML.
    pub xml_len: u32,
    /// Byte offset of the XML text.
    pub xml_offset: u64,
    /// Byte offset of the first memory block header (LIF/LIFEXT) or of the payload header (LOF).
    pub first_block_offset: u64,
    /// `LMSDataContainerHeader/@Version` (LIF, LOF) or `LMSDataContainerEnhancedHeader/@Version`.
    pub container_version: u32,
    /// The two u32 values after `LMS_Object_File` in a LOF (`None` for LIF/LIFEXT).
    pub lof_versions: Option<[u32; 2]>,
}

/// One memory block.
#[derive(Debug, Clone)]
pub struct MemoryBlock {
    /// Id string from the block header, e.g. `MemBlock_86` (LOF: taken from the XML).
    pub block_id: String,
    /// Byte offset of the block header.
    pub header_offset: u64,
    /// Byte offset of the payload.
    pub data_offset: u64,
    /// Payload length in bytes.
    pub data_len: u64,
}

/// An opened binary container: header, XML text, and the block table.
#[derive(Debug)]
pub struct LifFile {
    pub path: PathBuf,
    pub kind: ContainerKind,
    pub file_len: u64,
    pub header: ContainerHeader,
    pub xml: String,
    pub blocks: Vec<MemoryBlock>,
    /// Set when the block walk hit the end of file early; `check` reports it.
    pub truncated_at: Option<u64>,
    /// Set when a block header was malformed; `check` reports it.
    pub bad_block_at: Option<(u64, String)>,
}

fn utf16_tag(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Header block check shared by the three kinds: returns the UTF-16 text bytes after it.
fn header_text(head: &[u8]) -> Option<&[u8]> {
    if head.len() < 13 {
        return None;
    }
    if le_u32(head, 0) != Some(MAGIC) || head[8] != TEXT_MARKER {
        return None;
    }
    let block_len = le_u32(head, 4)?;
    let xml_len = le_u32(head, 9)?;
    if u64::from(block_len) != 2 * u64::from(xml_len) + 5 {
        return None;
    }
    Some(&head[13..])
}

/// Quick signature test on the first bytes: a LIF file.
pub fn looks_like_lif(head: &[u8]) -> bool {
    header_text(head).is_some_and(|t| t.starts_with(&utf16_tag("<LMSDataContainerHeader")))
}

/// Quick signature test on the first bytes: a LIFEXT sidecar.
pub fn looks_like_lifext(head: &[u8]) -> bool {
    header_text(head).is_some_and(|t| t.starts_with(&utf16_tag("<LMSDataContainerEnhancedHeader")))
}

/// Quick signature test on the first bytes: a LOF file.
pub fn looks_like_lof(head: &[u8]) -> bool {
    header_text(head).is_some_and(|t| t.starts_with(&utf16_tag(LOF_TEXT)))
}

pub(crate) fn read_head(fs: &Fs, path: &Path, n: usize) -> Result<Vec<u8>> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let mut head = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        let k = f
            .read(&mut head[filled..])
            .map_err(|e| Error::io(path, e))?;
        if k == 0 {
            break;
        }
        filled += k;
    }
    head.truncate(filled);
    Ok(head)
}

fn read_exact_at(f: &mut SourceFile, path: &Path, off: u64, buf: &mut [u8]) -> Result<()> {
    f.seek(SeekFrom::Start(off))
        .map_err(|e| Error::io(path, e))?;
    f.read_exact(buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::corrupt_at(
                FORMAT_ID,
                off,
                format!("file ends inside a {}-byte read", buf.len()),
            )
        } else {
            Error::io(path, e)
        }
    })
}

/// Reads one `0x70 u32 0x2A u32 utf16` text block at `off`; returns (block_len, text_len, text).
fn read_text_block(
    f: &mut SourceFile,
    path: &Path,
    off: u64,
    file_len: u64,
    what: &str,
) -> Result<(u32, u32, String)> {
    let mut h = [0u8; 13];
    read_exact_at(f, path, off, &mut h)?;
    if le_u32(&h, 0) != Some(MAGIC) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!("missing 0x70 block marker before the {what}"),
        ));
    }
    if h[8] != TEXT_MARKER {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off + 8,
            format!("missing 0x2A text marker before the {what}"),
        ));
    }
    let block_len = le_u32(&h, 4).unwrap_or(0);
    let text_len = le_u32(&h, 9).unwrap_or(0);
    if u64::from(block_len) != 2 * u64::from(text_len) + 5 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off + 4,
            format!("{what} block_len {block_len} != 2 * text length {text_len} + 5"),
        ));
    }
    let bytes = 2 * u64::from(text_len);
    if off.saturating_add(13).saturating_add(bytes) > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off + 13,
            format!("{what} extends past end of file (truncated file)"),
        ));
    }
    let len = checked_metadata_len(FORMAT_ID, bytes, remaining(file_len, off + 13), what)?;
    let mut tb = vec![0u8; len];
    read_exact_at(f, path, off + 13, &mut tb)?;
    Ok((block_len, text_len, utf16le(&tb)))
}

impl LifFile {
    /// Read the header, the XML, and walk the block chain. Never reads payloads.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let (block_len, xml_len, text) = read_text_block(&mut f, path, 0, file_len, "XML header")?;
        if text == LOF_TEXT {
            return Self::open_lof(f, path, file_len);
        }
        let kind = if text.starts_with("<LMSDataContainerEnhancedHeader") {
            ContainerKind::Lifext
        } else {
            ContainerKind::Lif
        };
        let xml = text;
        let container_version = parse_version(&xml);
        let first_block_offset = 13 + 2 * u64::from(xml_len);
        // LIFEXT sidecars use the 64-bit block layout whatever their header version says.
        let wide = kind == ContainerKind::Lifext || container_version >= 2;
        let (blocks, truncated_at, bad_block_at) =
            walk_blocks(&mut f, path, first_block_offset, file_len, wide)?;
        Ok(LifFile {
            path: path.to_path_buf(),
            kind,
            file_len,
            header: ContainerHeader {
                block_len,
                xml_len,
                xml_offset: 13,
                first_block_offset,
                container_version,
                lof_versions: None,
            },
            xml,
            blocks,
            truncated_at,
            bad_block_at,
        })
    }

    fn open_lof(mut f: SourceFile, path: &Path, file_len: u64) -> Result<Self> {
        // After the 13 + 30 byte `LMS_Object_File` block: 0x2A u32 0x2A u32 (two versions),
        // then 0x2A u64 payload length, the payload, then a 0x70 text block with the XML.
        let vo = 13 + 2 * LOF_TEXT.len() as u64;
        let mut vb = [0u8; 19];
        read_exact_at(&mut f, path, vo, &mut vb)?;
        if vb[0] != TEXT_MARKER || vb[5] != TEXT_MARKER {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                vo,
                "LOF: missing 0x2A markers around the version numbers",
            ));
        }
        let lof_versions = [le_u32(&vb, 1).unwrap_or(0), le_u32(&vb, 6).unwrap_or(0)];
        if vb[10] != TEXT_MARKER {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                vo + 10,
                "LOF: missing 0x2A marker before the payload length",
            ));
        }
        let data_len = le_u64(&vb, 11).unwrap_or(0);
        let header_offset = vo + 10;
        let data_offset = vo + 19;
        let xml_block = data_offset.checked_add(data_len).filter(|&e| e <= file_len);
        let Some(xml_block) = xml_block else {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                data_offset,
                format!(
                    "LOF payload of {data_len} bytes runs past end of file ({file_len} bytes); the XML that follows it is missing (truncated file)"
                ),
            ));
        };
        let (block_len, xml_len, mut xml) =
            read_text_block(&mut f, path, xml_block, file_len, "LOF XML")?;
        if xml.trim_start().starts_with("<Data>") {
            // Older LOF files carry a bare `<Data><Image>` fragment; wrap it so the image walk sees
            // an element named after the file (liffile documents the same fallback).
            let name = path.file_stem().map_or_else(
                || "Unnamed".to_string(),
                |s| s.to_string_lossy().to_string(),
            );
            xml = format!(
                "<LMSDataContainerHeader Version=\"2\"><Element Name=\"{}\">{xml}</Element></LMSDataContainerHeader>",
                xml_escape(&name)
            );
        }
        let container_version = parse_version(&xml);
        let block_id = memory_block_id_of(&xml).unwrap_or_else(|| "MemBlock_0".into());
        let end = xml_block + 13 + 2 * u64::from(xml_len);
        let bad_block_at =
            (end != file_len).then(|| (end, format!("{} bytes after the LOF XML", file_len - end)));
        Ok(LifFile {
            path: path.to_path_buf(),
            kind: ContainerKind::Lof,
            file_len,
            header: ContainerHeader {
                block_len,
                xml_len,
                xml_offset: xml_block + 13,
                first_block_offset: header_offset,
                container_version,
                lof_versions: Some(lof_versions),
            },
            xml,
            blocks: vec![MemoryBlock {
                block_id,
                header_offset,
                data_offset,
                data_len,
            }],
            truncated_at: None,
            bad_block_at,
        })
    }

    pub fn block(&self, id: &str) -> Option<&MemoryBlock> {
        if self.kind == ContainerKind::Lof {
            // A LOF holds exactly one payload, whatever id the XML gives it.
            return self.blocks.first();
        }
        self.blocks.iter().find(|b| b.block_id == id)
    }
}

type BlockWalk = (Vec<MemoryBlock>, Option<u64>, Option<(u64, String)>);

fn walk_blocks(
    f: &mut SourceFile,
    path: &Path,
    start: u64,
    file_len: u64,
    wide: bool,
) -> Result<BlockWalk> {
    let mut blocks = Vec::new();
    let mut off = start;
    let mut truncated_at = None;
    let mut bad_block_at = None;
    let fixed = if wide { 22usize } else { 18usize };
    while off < file_len {
        if off + fixed as u64 > file_len {
            truncated_at = Some(off);
            break;
        }
        let mut hb = vec![0u8; fixed];
        read_exact_at(f, path, off, &mut hb)?;
        if le_u32(&hb, 0) != Some(MAGIC) || hb[8] != TEXT_MARKER {
            bad_block_at = Some((off, "block marker mismatch".into()));
            break;
        }
        let blen = u64::from(le_u32(&hb, 4).unwrap_or(0));
        let (data_len, m3, id_len_pos) = if wide {
            (le_u64(&hb, 9).unwrap_or(0), hb[17], 18)
        } else {
            (u64::from(le_u32(&hb, 9).unwrap_or(0)), hb[13], 14)
        };
        if m3 != TEXT_MARKER {
            bad_block_at = Some((off, "missing 0x2A before block id".into()));
            break;
        }
        let id_len = u64::from(le_u32(&hb, id_len_pos).unwrap_or(0));
        let expect = 2 * id_len + if wide { 14 } else { 10 };
        if blen != expect {
            bad_block_at = Some((
                off,
                format!("block_len {blen} != 2 * id_len {id_len} + fixed ({expect})"),
            ));
            break;
        }
        let id_off = off + fixed as u64;
        if id_off + 2 * id_len > file_len {
            truncated_at = Some(off);
            break;
        }
        // Block ids are short names ("MemBlock_86"); a huge id is a corrupt header.
        let Ok(id_bytes) = usize::try_from(2 * id_len).map_err(drop).and_then(|n| {
            if id_len > MAX_BLOCK_ID_UNITS {
                Err(())
            } else {
                Ok(n)
            }
        }) else {
            bad_block_at = Some((off, format!("block id length {id_len} is implausible")));
            break;
        };
        let mut idb = vec![0u8; id_bytes];
        read_exact_at(f, path, id_off, &mut idb)?;
        let block_id = utf16le(&idb);
        let data_offset = id_off + 2 * id_len;
        blocks.push(MemoryBlock {
            block_id,
            header_offset: off,
            data_offset,
            data_len,
        });
        match data_offset.checked_add(data_len) {
            Some(next) if next <= file_len => off = next,
            _ => {
                truncated_at = Some(data_offset);
                break;
            }
        }
    }
    Ok((blocks, truncated_at, bad_block_at))
}

/// Longest block id accepted, in UTF-16 code units.
const MAX_BLOCK_ID_UNITS: u64 = 4096;

fn parse_version(xml: &str) -> u32 {
    let head: String = xml.chars().take(400).collect();
    head.find("Version=\"")
        .and_then(|i| head[i + 9..].split('"').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn memory_block_id_of(xml: &str) -> Option<String> {
    let i = xml.find("MemoryBlockID=\"")?;
    xml[i + 15..].split('"').next().map(str::to_string)
}

pub(crate) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_block(text: &str) -> Vec<u8> {
        let units: Vec<u8> = utf16_tag(text);
        let n = u32::try_from(units.len() / 2).unwrap();
        let mut v = Vec::new();
        v.extend_from_slice(&MAGIC.to_le_bytes());
        v.extend_from_slice(&(2 * n + 5).to_le_bytes());
        v.push(TEXT_MARKER);
        v.extend_from_slice(&n.to_le_bytes());
        v.extend_from_slice(&units);
        v
    }

    #[test]
    fn signatures() {
        let lif = text_block("<LMSDataContainerHeader Version=\"2\"></LMSDataContainerHeader>");
        let ext = text_block("<LMSDataContainerEnhancedHeader Version=\"1\"/>");
        let lof = text_block(LOF_TEXT);
        assert!(looks_like_lif(&lif) && !looks_like_lifext(&lif) && !looks_like_lof(&lif));
        assert!(looks_like_lifext(&ext) && !looks_like_lif(&ext));
        assert!(looks_like_lof(&lof) && !looks_like_lif(&lof));
        assert!(!looks_like_lif(&lif[..10]));
    }

    #[test]
    fn lof_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lif-lof-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.lof");
        let mut v = text_block(LOF_TEXT);
        v.push(TEXT_MARKER);
        v.extend_from_slice(&2u32.to_le_bytes());
        v.push(TEXT_MARKER);
        v.extend_from_slice(&1u32.to_le_bytes());
        v.push(TEXT_MARKER);
        v.extend_from_slice(&4u64.to_le_bytes());
        v.extend_from_slice(&[1, 2, 3, 4]);
        v.extend_from_slice(&text_block("<Data><Image/></Data>"));
        std::fs::write(&p, &v).unwrap();
        let f = LifFile::open(&p).unwrap();
        assert_eq!(f.kind, ContainerKind::Lof);
        assert_eq!(f.header.lof_versions, Some([2, 1]));
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(f.blocks[0].data_len, 4);
        assert!(
            f.xml
                .contains("<Element Name=\"x\"><Data><Image/></Data></Element>")
        );
        assert!(f.bad_block_at.is_none());
        // truncated payload: clean corrupt error, no panic
        std::fs::write(&p, &v[..v.len() - 30]).unwrap();
        assert!(matches!(LifFile::open(&p), Err(Error::Corrupt { .. })));
        std::fs::remove_dir_all(&dir).ok();
    }
}
