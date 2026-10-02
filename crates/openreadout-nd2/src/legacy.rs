//! Legacy ND2 (NIS-Elements 2.x): a JPEG 2000 box sequence whose frames are `jp2c` codestreams
//! and whose metadata are XML boxes, indexed by a box map at the end of the file.
//! See `docs/formats/nd2.md` § Legacy container.

use std::path::{Path, PathBuf};

use openreadout_core::bytes::{be_u16, be_u32, be_u64, le_u64};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};
use serde_json::Value;

use crate::FORMAT_ID;
use crate::container::read_at;
use crate::variant::legacy_xml_decode;

/// Signature in the 32 bytes before the trailing box-map distance.
pub const BOX_MAP_SIGNATURE: &[u8; 32] = b"LABORATORY IMAGING ND BOX MAP 00";

/// One entry of the box map: a top-level box, its four-character type and the tag that says
/// what it holds (`LUNK` frame codestreams, `VIMD` per-frame metadata, …).
#[derive(Debug, Clone)]
pub struct LegacyBox {
    pub box_type: String,
    pub tag: String,
    pub box_offset: u64,
}

/// An opened legacy ND2 container.
#[derive(Debug)]
pub struct LegacyFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub boxes: Vec<LegacyBox>,
    /// Offset of the box map, when it was found.
    pub box_map_offset: Option<u64>,
    /// True when the box map was rebuilt by walking the boxes.
    pub rescued: bool,
    pub problems: Vec<(Option<u64>, String)>,
}

/// (payload offset, payload length, box type) of the box at `off`.
pub(crate) fn box_header(
    f: &mut SourceFile,
    path: &Path,
    off: u64,
    file_len: u64,
) -> Result<(u64, u64, String)> {
    if off.saturating_add(8) > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            "box header past end of file",
        ));
    }
    let h = read_at(f, path, off, 8)?;
    let len = u64::from(be_u32(&h, 0).unwrap_or(0));
    let ty = String::from_utf8_lossy(&h[4..8]).to_string();
    let (payload, total) = match len {
        0 => (off + 8, file_len - off),
        1 => {
            let x = read_at(f, path, off + 8, 8)?;
            (off + 16, be_u64(&x, 0).unwrap_or(0))
        }
        n => (off + 8, n),
    };
    let header_len = payload - off;
    if total < header_len || off.saturating_add(total) > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!("box '{ty}' of {total} bytes runs past the end of the file (truncated?)"),
        ));
    }
    Ok((payload, total - header_len, ty))
}

/// Tag of an XML box, from its root element, for files whose box map is lost.
fn tag_from_xml_root(xml: &[u8]) -> &'static str {
    let s = String::from_utf8_lossy(&xml[..xml.len().min(512)]).to_string();
    let root = s
        .split('<')
        .map(str::trim_start)
        .find(|t| !t.starts_with('!') && !t.starts_with('?') && !t.is_empty())
        .map(|t| {
            t.split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();
    match root.as_str() {
        "CalibrationSeq" => "VCAL",
        "MetadataSeq" => "VIMD",
        "AdvancedImageAttributes" => "ARTT",
        "Calibration" => "ACAL",
        "TextInfo" => "TINF",
        "Events" => "IEVE",
        "Metadata" => "AIMD",
        r if r.starts_with("Metadata_V") => "AIM1",
        "ReportObjects" => "SROB",
        _ => "????",
    }
}

impl LegacyFile {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        if file_len < 64 {
            return Err(Error::corrupt(FORMAT_ID, "file too small to be an ND2"));
        }
        let mut problems = Vec::new();
        let tail = read_at(&mut f, path, file_len - 40, 40)?;
        let mut boxes = Vec::new();
        let mut box_map_offset = None;
        if &tail[..32] == BOX_MAP_SIGNATURE {
            let dist = le_u64(&tail, 32).unwrap_or(0);
            if dist >= 44 && dist <= file_len {
                let start = file_len - dist;
                let map = read_at(&mut f, path, start, (dist - 40) as usize)?;
                let n = be_u32(&map, 0).unwrap_or(0) as usize;
                if 4 + n.saturating_mul(16) <= map.len() {
                    for i in 0..n {
                        let e = &map[4 + 16 * i..4 + 16 * i + 16];
                        boxes.push(LegacyBox {
                            box_type: String::from_utf8_lossy(&e[..4]).to_string(),
                            tag: String::from_utf8_lossy(&e[4..8]).to_string(),
                            box_offset: le_u64(e, 8).unwrap_or(0),
                        });
                    }
                    box_map_offset = Some(start);
                } else {
                    problems.push((
                        Some(start),
                        format!("box map claims {n} entries but holds fewer"),
                    ));
                }
            } else {
                problems.push((Some(file_len - 8), "box-map distance out of range".into()));
            }
        } else {
            problems.push((
                Some(file_len - 40),
                "box-map signature missing at the end of the file".into(),
            ));
        }
        let mut rescued = false;
        if boxes.iter().all(|b| b.box_type != "jp2c") {
            boxes = Self::walk_boxes(&mut f, path, file_len)?;
            rescued = true;
            problems.push((
                None,
                format!(
                    "box map unusable; recovered {} boxes by walking the file (interrupted acquisition?)",
                    boxes.len()
                ),
            ));
        }
        if !boxes.iter().any(|b| b.box_type == "jp2c") {
            return Err(Error::corrupt(
                FORMAT_ID,
                "legacy ND2 without any JPEG 2000 codestream box",
            ));
        }
        Ok(LegacyFile {
            path: path.to_path_buf(),
            file_len,
            boxes,
            box_map_offset,
            rescued,
            problems,
        })
    }

    /// Rebuild the box list by walking top-level boxes from the start of the file.
    fn walk_boxes(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<Vec<LegacyBox>> {
        let mut out = Vec::new();
        let mut off = 0u64;
        while off + 8 <= file_len {
            let Ok((payload, len, ty)) = box_header(f, path, off, file_len) else {
                break;
            };
            let tag = match ty.as_str() {
                "xml " => {
                    let head = read_at(f, path, payload, len.min(512) as usize)?;
                    tag_from_xml_root(&head).to_string()
                }
                "uuid" => "PMAP".into(),
                _ => "LUNK".into(),
            };
            out.push(LegacyBox {
                box_type: ty,
                tag,
                box_offset: off,
            });
            let next = payload + len;
            if next <= off {
                break;
            }
            off = next;
        }
        Ok(out)
    }

    /// XML boxes with this tag, in map order.
    pub fn tagged(&self, tag: &str) -> Vec<&LegacyBox> {
        self.boxes
            .iter()
            .filter(|b| b.tag == tag && b.box_type == "xml ")
            .collect()
    }

    /// Frame codestream boxes (`jp2c`) in map order.
    pub fn codestreams(&self) -> Vec<&LegacyBox> {
        self.boxes.iter().filter(|b| b.box_type == "jp2c").collect()
    }

    pub fn read_box(&self, f: &mut SourceFile, b: &LegacyBox) -> Result<Vec<u8>> {
        let (payload, len, ty) = box_header(f, &self.path, b.box_offset, self.file_len)?;
        if ty != b.box_type {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                b.box_offset,
                format!("box map says '{}' but the box is '{ty}'", b.box_type),
            ));
        }
        let len = usize::try_from(len)
            .map_err(|_| Error::corrupt_at(FORMAT_ID, b.box_offset, "box too large"))?;
        read_at(f, &self.path, payload, len)
    }

    /// The `n`-th XML box with `tag`, decoded.
    pub fn xml(&self, f: &mut SourceFile, tag: &str, n: usize) -> Option<Value> {
        let b = self.tagged(tag).into_iter().nth(n)?;
        let raw = self.read_box(f, b).ok()?;
        legacy_xml_decode(&raw).ok()
    }

    /// `ihdr` of the `jp2h` header box: (height, width, components, bits per component).
    pub fn image_header(&self, f: &mut SourceFile) -> Result<(u32, u32, u16, u8)> {
        let jp2h = self
            .boxes
            .iter()
            .find(|b| b.box_type == "jp2h")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "no jp2h header box"))?;
        let body = self.read_box(f, jp2h)?;
        if body.len() < 22 || &body[4..8] != b"ihdr" {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                jp2h.box_offset,
                "jp2h does not start with an ihdr box",
            ));
        }
        let h = be_u32(&body, 8).unwrap_or(0);
        let w = be_u32(&body, 12).unwrap_or(0);
        let nc = be_u16(&body, 16).unwrap_or(0);
        let bpc = (body[18] & 0x7F) + 1;
        Ok((h, w, nc, bpc))
    }
}

/// Image size and component count declared by a codestream's SIZ marker (checked before
/// decoding so a damaged header cannot request a huge allocation).
pub fn codestream_size(cs: &[u8]) -> Option<(u32, u32, u16)> {
    if cs.len() < 42 || cs[..4] != [0xFF, 0x4F, 0xFF, 0x51] {
        return None;
    }
    let x = be_u32(cs, 8)?.checked_sub(be_u32(cs, 16)?)?;
    let y = be_u32(cs, 12)?.checked_sub(be_u32(cs, 20)?)?;
    let c = be_u16(cs, 40)?;
    Some((x, y, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_roots_map_to_tags() {
        assert_eq!(
            tag_from_xml_root(b"<!--x-->\r\n<CalibrationSeq _SEQUENCE_INDEX=\"0\">"),
            "VCAL"
        );
        assert_eq!(
            tag_from_xml_root(b"<Metadata_V1.2 version=\"1.0\">"),
            "AIM1"
        );
        assert_eq!(tag_from_xml_root(b"<Metadata version=\"1.0\">"), "AIMD");
    }

    #[test]
    fn siz_marker() {
        let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51, 0, 41, 0, 0];
        cs.extend_from_slice(&696u32.to_be_bytes());
        cs.extend_from_slice(&520u32.to_be_bytes());
        cs.extend_from_slice(&[0u8; 8]);
        cs.extend_from_slice(&[0u8; 16]);
        cs.extend_from_slice(&1u16.to_be_bytes());
        assert_eq!(codestream_size(&cs), Some((696, 520, 1)));
        assert_eq!(codestream_size(&cs[..20]), None);
    }
}
