//! Where an image's memory block lives on disk: one or more byte ranges ("segments") in one or
//! more files. A LIF/LIFEXT/LOF block is one segment; an XLIF block is one segment per frame.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::frames::{FrameDecode, frame_bytes};

/// `len` bytes of the block starting at `virt_offset` are stored at `file_offset` of `file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    pub file: usize,
    pub file_offset: u64,
    pub virt_offset: u64,
    pub len: u64,
}

/// The byte ranges that make up one memory block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Storage {
    pub segments: Vec<Segment>,
}

impl Storage {
    pub fn single(file: usize, file_offset: u64, len: u64) -> Self {
        Storage {
            segments: vec![Segment {
                file,
                file_offset,
                virt_offset: 0,
                len,
            }],
        }
    }

    /// Number of block bytes stored contiguously from offset 0.
    pub fn contiguous_len(&self) -> u64 {
        let mut segs: Vec<&Segment> = self.segments.iter().collect();
        segs.sort_by_key(|s| s.virt_offset);
        let mut end = 0u64;
        for s in segs {
            if s.virt_offset > end {
                break;
            }
            end = end.max(s.virt_offset.saturating_add(s.len));
        }
        end
    }

    /// File and file offset of block offset `virt`, if it is stored.
    pub fn locate(&self, virt: u64) -> Option<(usize, u64)> {
        self.segments
            .iter()
            .find(|s| virt >= s.virt_offset && virt - s.virt_offset < s.len)
            .map(|s| (s.file, s.file_offset + (virt - s.virt_offset)))
    }
}

/// Open files by index, lazily. A file registered with [`FileTable::index_of_frame`] is an
/// XLIF frame image: reads come from its decoded samples, not its bytes.
#[derive(Debug, Default)]
pub(crate) struct FileTable {
    pub paths: Vec<PathBuf>,
    handles: Vec<Option<SourceFile>>,
    /// Per file: how to decode it when it is an image-file frame.
    frames: Vec<Option<FrameDecode>>,
    /// Decoded frame bytes, kept after the first read.
    decoded: Vec<Option<std::sync::Arc<Vec<u8>>>>,
    /// Where the files are read from.
    pub(crate) fs: Fs,
}

impl FileTable {
    /// An empty table reading from `fs`.
    pub(crate) fn new(fs: Fs) -> Self {
        Self {
            fs,
            ..Self::default()
        }
    }

    pub fn index_of(&mut self, path: &std::path::Path) -> usize {
        if let Some(i) = self
            .paths
            .iter()
            .zip(&self.frames)
            .position(|(p, f)| p == path && f.is_none())
        {
            return i;
        }
        self.push(path, None)
    }

    /// Register an image file holding an XLIF frame, decoded as `spec` says.
    pub fn index_of_frame(&mut self, path: &std::path::Path, spec: FrameDecode) -> usize {
        if let Some(i) = self
            .paths
            .iter()
            .zip(&self.frames)
            .position(|(p, f)| p == path && f.as_ref() == Some(&spec))
        {
            return i;
        }
        self.push(path, Some(spec))
    }

    fn push(&mut self, path: &std::path::Path, frame: Option<FrameDecode>) -> usize {
        self.paths.push(path.to_path_buf());
        self.handles.push(None);
        self.frames.push(frame);
        self.decoded.push(None);
        self.paths.len() - 1
    }

    /// Whether file `i` is an image-file frame, and of which kind.
    pub fn frame_kind(&self, i: usize) -> Option<crate::frames::FrameKind> {
        self.frames.get(i)?.as_ref().map(|f| f.kind)
    }

    /// The decoded bytes of image-file frame `i`.
    fn decoded(&mut self, i: usize) -> Result<std::sync::Arc<Vec<u8>>> {
        if let Some(d) = &self.decoded[i] {
            return Ok(d.clone());
        }
        let spec = self.frames[i].clone().expect("a frame file");
        let d = std::sync::Arc::new(frame_bytes(&self.fs, &self.paths[i], &spec)?);
        self.decoded[i] = Some(d.clone());
        Ok(d)
    }

    fn handle(&mut self, i: usize) -> Result<&mut SourceFile> {
        let path = self.paths[i].clone();
        let slot = &mut self.handles[i];
        if slot.is_none() {
            *slot = Some(self.fs.open(&path).map_err(|e| Error::io(&path, e))?);
        }
        Ok(slot.as_mut().expect("just opened"))
    }

    /// Fill `buf` with block bytes starting at block offset `virt`.
    pub fn read(&mut self, storage: &Storage, virt: u64, buf: &mut [u8]) -> Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let at = virt + done as u64;
            let seg = storage
                .segments
                .iter()
                .find(|s| at >= s.virt_offset && at - s.virt_offset < s.len)
                .copied()
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "byte {at} of the memory block is not stored in the file (file truncated or frame missing?)"
                        ),
                    )
                })?;
            let within = at - seg.virt_offset;
            let avail = usize::try_from(seg.len - within).unwrap_or(usize::MAX);
            let n = avail.min(buf.len() - done);
            if self.frames[seg.file].is_some() {
                let d = self.decoded(seg.file)?;
                let from = usize::try_from(seg.file_offset + within).unwrap_or(usize::MAX);
                let src = from
                    .checked_add(n)
                    .and_then(|end| d.get(from..end))
                    .ok_or_else(|| {
                        Error::corrupt(FORMAT_ID, "frame image is smaller than its XLIF frame")
                    })?;
                buf[done..done + n].copy_from_slice(src);
                done += n;
                continue;
            }
            let path = self.paths[seg.file].clone();
            let f = self.handle(seg.file)?;
            f.seek(SeekFrom::Start(seg.file_offset + within))
                .map_err(|e| Error::io(&path, e))?;
            f.read_exact(&mut buf[done..done + n]).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    Error::corrupt_at(
                        FORMAT_ID,
                        seg.file_offset + within,
                        "file ends inside a memory block (truncated file)",
                    )
                } else {
                    Error::io(&path, e)
                }
            })?;
            done += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        let s = Storage {
            segments: vec![
                Segment {
                    file: 0,
                    file_offset: 100,
                    virt_offset: 0,
                    len: 10,
                },
                Segment {
                    file: 1,
                    file_offset: 7,
                    virt_offset: 10,
                    len: 5,
                },
                Segment {
                    file: 1,
                    file_offset: 0,
                    virt_offset: 20,
                    len: 5,
                },
            ],
        };
        assert_eq!(s.contiguous_len(), 15);
        assert_eq!(s.locate(3), Some((0, 103)));
        assert_eq!(s.locate(12), Some((1, 9)));
        assert_eq!(s.locate(17), None);
    }

    #[test]
    fn reads_across_segments() {
        let dir = std::env::temp_dir().join(format!("lif-storage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        std::fs::write(&a, [0u8, 1, 2, 3, 4, 5]).unwrap();
        std::fs::write(&b, [10u8, 11, 12, 13]).unwrap();
        let mut t = FileTable::default();
        let ia = t.index_of(&a);
        let ib = t.index_of(&b);
        assert_eq!(t.index_of(&a), ia);
        let s = Storage {
            segments: vec![
                Segment {
                    file: ia,
                    file_offset: 2,
                    virt_offset: 0,
                    len: 3,
                },
                Segment {
                    file: ib,
                    file_offset: 1,
                    virt_offset: 3,
                    len: 3,
                },
            ],
        };
        let mut buf = [0u8; 5];
        t.read(&s, 1, &mut buf).unwrap();
        assert_eq!(buf, [3, 4, 11, 12, 13]);
        let mut buf = [0u8; 4];
        assert!(t.read(&s, 4, &mut buf).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
