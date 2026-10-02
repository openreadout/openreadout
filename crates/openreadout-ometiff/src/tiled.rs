//! Tiled, pyramidal BigTIFF writer for OME-TIFF: one main IFD per plane (full resolution) whose
//! `SubIFDs` tag lists the plane's reduced-resolution levels, as the OME-TIFF specification
//! describes sub-resolutions. Tiles arrive as blocks from [`crate::pyramid::stream_plane`] and are
//! compressed on worker threads and appended to the file in block order; the IFDs follow each
//! plane's data. Little-endian, planar configuration 1 (interleaved samples).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use openreadout_core::{Error, Plane, Result};
use rayon::prelude::*;

use crate::pyramid::BlockSink;
use crate::{Codec, SampleLayout, compression_tag};

const T_SHORT: u16 = 3;
const T_LONG: u16 = 4;
const T_ASCII: u16 = 2;
const T_LONG8: u16 = 16;
const T_IFD8: u16 = 18;

/// One IFD entry: tag, type, and the value bytes (little-endian).
struct Entry {
    tag: u16,
    ty: u16,
    count: u64,
    bytes: Vec<u8>,
}

fn shorts(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        ty: T_SHORT,
        count: v.len() as u64,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        ty: T_LONG,
        count: 1,
        bytes: v.to_le_bytes().to_vec(),
    }
}
fn long8s(tag: u16, ty: u16, v: &[u64]) -> Entry {
    Entry {
        tag,
        ty,
        count: v.len() as u64,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    Entry {
        tag,
        ty: T_ASCII,
        count: b.len() as u64,
        bytes: b,
    }
}

/// The tiles of one level of the plane being written.
struct LevelTiles {
    width: u32,
    height: u32,
    across: u32,
    offsets: Vec<u64>,
    counts: Vec<u64>,
}

/// A BigTIFF being written.
pub(crate) struct TiledTiff {
    out: BufWriter<File>,
    pos: u64,
    /// File offset of the `next IFD` field to patch with the next main IFD's offset.
    chain: u64,
    tile: u32,
    codec: Codec,
    layout: SampleLayout,
    spp: u16,
    bytes_per_sample: usize,
    levels: Vec<LevelTiles>,
    path: std::path::PathBuf,
}

impl TiledTiff {
    pub(crate) fn create(
        path: &Path,
        tile: u32,
        codec: Codec,
        layout: SampleLayout,
        spp: u16,
        bytes_per_sample: usize,
    ) -> Result<Self> {
        if tile == 0 || !tile.is_multiple_of(16) {
            return Err(Error::Usage(format!(
                "TIFF tile size {tile} must be a positive multiple of 16"
            )));
        }
        let f = File::create(path).map_err(|e| Error::io(path, e))?;
        let mut t = TiledTiff {
            out: BufWriter::with_capacity(1 << 20, f),
            pos: 0,
            chain: 8,
            tile,
            codec,
            layout,
            spp,
            bytes_per_sample,
            levels: Vec::new(),
            path: path.to_path_buf(),
        };
        // BigTIFF header: "II", 43, offset size 8, 0, first IFD offset (patched later).
        let mut h = Vec::with_capacity(16);
        h.extend_from_slice(b"II");
        h.extend_from_slice(&43u16.to_le_bytes());
        h.extend_from_slice(&8u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&0u64.to_le_bytes());
        t.write(&h)?;
        Ok(t)
    }

    fn io(&self, e: std::io::Error) -> Error {
        Error::io(&self.path, e)
    }

    fn write(&mut self, b: &[u8]) -> Result<u64> {
        let at = self.pos;
        self.out
            .write_all(b)
            .map_err(|e| Error::io(&self.path, e))?;
        self.pos += b.len() as u64;
        Ok(at)
    }

    /// Start a plane with these level sizes (full resolution first).
    pub(crate) fn begin_plane(&mut self, sizes: &[(u32, u32)]) {
        self.levels = sizes
            .iter()
            .map(|&(w, h)| {
                let across = w.div_ceil(self.tile);
                let n = across as usize * h.div_ceil(self.tile) as usize;
                LevelTiles {
                    width: w,
                    height: h,
                    across,
                    offsets: vec![0; n],
                    counts: vec![0; n],
                }
            })
            .collect();
    }

    /// Write the IFDs of the plane: the reduced levels first (unlinked, listed in `SubIFDs`), then
    /// the full-resolution IFD linked into the main chain. `description` goes into the first one.
    pub(crate) fn end_plane(&mut self, description: Option<&str>, software: &str) -> Result<u64> {
        let levels = std::mem::take(&mut self.levels);
        if levels.iter().any(|l| l.counts.contains(&0)) {
            return Err(Error::Other(
                "internal error: a tile of the plane was never written".into(),
            ));
        }
        let mut subs = Vec::new();
        for l in &levels[1..] {
            let (off, _) = self.write_ifd(l, 1, None, software, &[])?;
            subs.push(off);
        }
        let (off, next_field) = self.write_ifd(&levels[0], 0, description, software, &subs)?;
        // Link: the previous `next IFD` field (or the header's first-IFD field) points here.
        self.patch_u64(self.chain, off)?;
        self.chain = next_field;
        Ok(off)
    }

    fn patch_u64(&mut self, at: u64, v: u64) -> Result<()> {
        self.out.flush().map_err(|e| self.io(e))?;
        let f = self.out.get_mut();
        f.seek(SeekFrom::Start(at))
            .map_err(|e| Error::io(&self.path, e))?;
        f.write_all(&v.to_le_bytes())
            .map_err(|e| Error::io(&self.path, e))?;
        f.seek(SeekFrom::Start(self.pos))
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(())
    }

    /// Write one IFD; returns its offset and the offset of its `next IFD` field.
    fn write_ifd(
        &mut self,
        l: &LevelTiles,
        subfile: u32,
        description: Option<&str>,
        software: &str,
        subs: &[u64],
    ) -> Result<(u64, u64)> {
        let spp = usize::from(self.spp);
        let mut e = vec![
            long(254, subfile),
            long(256, l.width),
            long(257, l.height),
            shorts(258, &vec![self.layout.bits; spp]),
            shorts(259, &[compression_tag(self.codec)]),
            shorts(262, &[self.layout.photometric]),
            shorts(277, &[self.spp]),
            shorts(284, &[1]),
            ascii(305, software),
            long(322, self.tile),
            long(323, self.tile),
            long8s(324, T_LONG8, &l.offsets),
            long8s(325, T_LONG8, &l.counts),
            shorts(339, &vec![self.layout.format; spp]),
        ];
        if let Some(d) = description {
            e.push(ascii(270, d));
        }
        if !subs.is_empty() {
            e.push(long8s(330, T_IFD8, subs));
        }
        e.sort_by_key(|x| x.tag);
        // Values longer than 8 bytes go before the IFD (word aligned).
        let mut vals = Vec::with_capacity(e.len());
        for x in &e {
            if x.bytes.len() > 8 {
                if self.pos % 2 == 1 {
                    self.write(&[0])?;
                }
                vals.push(Some(self.write(&x.bytes)?));
            } else {
                vals.push(None);
            }
        }
        if self.pos % 2 == 1 {
            self.write(&[0])?;
        }
        let mut ifd = Vec::with_capacity(8 + e.len() * 20 + 8);
        ifd.extend_from_slice(&(e.len() as u64).to_le_bytes());
        for (x, v) in e.iter().zip(&vals) {
            ifd.extend_from_slice(&x.tag.to_le_bytes());
            ifd.extend_from_slice(&x.ty.to_le_bytes());
            ifd.extend_from_slice(&x.count.to_le_bytes());
            let mut field = [0u8; 8];
            match v {
                Some(off) => field.copy_from_slice(&off.to_le_bytes()),
                None => field[..x.bytes.len()].copy_from_slice(&x.bytes),
            }
            ifd.extend_from_slice(&field);
        }
        ifd.extend_from_slice(&0u64.to_le_bytes());
        let off = self.write(&ifd)?;
        Ok((off, off + ifd.len() as u64 - 8))
    }

    /// Flush and close the file.
    pub(crate) fn finish(mut self) -> Result<u64> {
        self.out.flush().map_err(|e| self.io(e))?;
        Ok(self.pos)
    }

    /// Cut, pad and compress the tiles covered by a block of `level` at `(x, y)`.
    /// Returns `(column, row, bytes)` of each tile in its level's tile grid.
    #[allow(clippy::many_single_char_names)]
    fn encode_block(&self, x: u32, y: u32, p: &Plane) -> Result<Vec<(usize, usize, Vec<u8>)>> {
        let t = self.tile as usize;
        let bpp = usize::from(self.spp) * self.bytes_per_sample;
        let row = p.width as usize * bpp;
        let across = p.width.div_ceil(self.tile) as usize;
        let down = p.height.div_ceil(self.tile) as usize;
        let tiles: Vec<(usize, usize)> = (0..down)
            .flat_map(|ty| (0..across).map(move |tx| (tx, ty)))
            .collect();
        let big_endian = cfg!(target_endian = "big");
        let bps = self.bytes_per_sample;
        let codec = self.codec;
        tiles
            .par_iter()
            .map(|&(tx, ty)| {
                let mut buf = vec![0u8; t * t * bpp];
                let (x0, y0) = (tx * t, ty * t);
                let cols = t.min(p.width as usize - x0) * bpp;
                for r in 0..t.min(p.height as usize - y0) {
                    let s = (y0 + r) * row + x0 * bpp;
                    buf[r * t * bpp..r * t * bpp + cols].copy_from_slice(&p.data[s..s + cols]);
                }
                if big_endian && bps > 1 {
                    for v in buf.chunks_mut(bps) {
                        v.reverse();
                    }
                }
                let data = match codec {
                    Codec::None => buf,
                    Codec::Deflate => {
                        let mut z = flate2::write::ZlibEncoder::new(
                            Vec::with_capacity(buf.len() / 2),
                            flate2::Compression::new(6),
                        );
                        z.write_all(&buf)
                            .and_then(|()| z.finish())
                            .map_err(|e| Error::Other(format!("deflate compression failed: {e}")))?
                    }
                    Codec::Lzw => {
                        weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
                            .encode(&buf)
                            .map_err(|e| Error::Other(format!("LZW compression failed: {e}")))?
                    }
                };
                // Index of the tile in its level's grid.
                let gx = x as usize / t + tx;
                let gy = y as usize / t + ty;
                Ok((gx, gy, data))
            })
            .collect::<Result<Vec<_>>>()
    }
}

impl BlockSink for TiledTiff {
    #[allow(clippy::many_single_char_names)]
    fn block(&mut self, level: usize, x: u32, y: u32, p: &Plane) -> Result<()> {
        if !x.is_multiple_of(self.tile) || !y.is_multiple_of(self.tile) {
            return Err(Error::Other(format!(
                "internal error: block at {x},{y} is not aligned to {}-pixel tiles",
                self.tile
            )));
        }
        let across = self
            .levels
            .get(level)
            .map(|l| l.across as usize)
            .ok_or_else(|| Error::Other(format!("internal error: no level {level}")))?;
        for (gx, gy, data) in self.encode_block(x, y, p)? {
            let i = gy * across + gx;
            let off = self.write(&data)?;
            let l = &mut self.levels[level];
            if i >= l.offsets.len() {
                return Err(Error::Other(format!(
                    "internal error: tile {gx},{gy} is outside level {level}"
                )));
            }
            l.offsets[i] = off;
            l.counts[i] = data.len() as u64;
        }
        Ok(())
    }
}
