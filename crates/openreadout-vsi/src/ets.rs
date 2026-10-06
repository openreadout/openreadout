//! ETS tile files (`_<name>_/stack<id>/frame_t*.ets`): SIS header, ETS header, tile table,
//! tile decoding. Layout (our derivation): `docs/formats/vsi.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{le_u32, le_u64};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, PixelType, Result};

use crate::FORMAT_ID;

/// First four bytes of an ETS file.
pub const SIS_MAGIC: &[u8; 4] = b"SIS\0";
/// First four bytes of the second header.
pub const ETS_MAGIC: &[u8; 4] = b"ETS\0";
/// Largest tile table we load (entries).
const MAX_TILES: u32 = 50_000_000;

/// How tiles are stored (ETS header word at +20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtsCompression {
    /// 0: raw little-endian samples.
    Raw,
    /// 2: baseline JPEG stream.
    Jpeg,
    /// 3: JPEG 2000 codestream.
    Jpeg2000,
    /// 5: lossless JPEG stream (SOF3), seen in VS120 slides.
    JpegLossless,
    /// Any other code (not seen in the corpus).
    Other(u32),
}

impl EtsCompression {
    pub fn from_code(c: u32) -> Self {
        match c {
            0 => EtsCompression::Raw,
            2 => EtsCompression::Jpeg,
            3 => EtsCompression::Jpeg2000,
            5 => EtsCompression::JpegLossless,
            other => EtsCompression::Other(other),
        }
    }
    pub fn name(self) -> String {
        match self {
            EtsCompression::Raw => "raw".into(),
            EtsCompression::Jpeg => "jpeg".into(),
            EtsCompression::Jpeg2000 => "jpeg2000".into(),
            EtsCompression::JpegLossless => "jpeg-lossless".into(),
            EtsCompression::Other(c) => format!("code {c}"),
        }
    }
}

/// The two headers of an ETS file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EtsHeader {
    /// Coordinates per tile (`n`: X, Y, the non-XY dimensions, the pyramid level).
    pub coord_count: u32,
    /// Offset and size of the second (`ETS`) header.
    pub ets_offset: u64,
    pub ets_size: u32,
    pub table_offset: u64,
    pub tile_count: u32,
    /// `0x00030006` in every corpus file.
    pub version: u32,
    /// 2 = 8-bit, 4 = 16-bit (unsigned).
    pub sample_type: u32,
    pub samples_per_pixel: u32,
    /// 1 in grayscale files, 4 in RGB files (meaning otherwise unknown).
    pub color_space: u32,
    pub compression: EtsCompression,
    pub quality: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tile_depth: u32,
    /// Value written for tiles that are not stored (`0xEEEEEE` light grey on slides).
    pub background: u32,
    /// X, Y, then the sizes of the non-XY dimensions.
    pub sizes: Vec<u32>,
}

impl EtsHeader {
    pub fn pixel_type(&self) -> Option<PixelType> {
        match self.sample_type {
            2 => Some(PixelType::Uint8),
            4 => Some(PixelType::Uint16),
            _ => None,
        }
    }

    /// Number of non-XY dimensions addressed by each tile.
    pub fn extra_dims(&self) -> usize {
        (self.coord_count as usize).saturating_sub(3)
    }

    /// Size of each tile table entry.
    pub fn entry_size(&self) -> u64 {
        4 + 4 * u64::from(self.coord_count) + 16
    }
}

/// One stored tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// `[column, row, non-XY indices..., level]`.
    pub coords: Vec<u32>,
    pub offset: u64,
    pub len: u32,
}

impl Tile {
    /// Tile column. Stored as a signed 32-bit number: stitched images (Stream "MIA") index
    /// tiles left of and above the grid origin with negative columns and rows.
    pub fn column(&self) -> i64 {
        i64::from(self.coords.first().copied().unwrap_or(0) as i32)
    }
    /// Tile row (signed, see [`Tile::column`]).
    pub fn row(&self) -> i64 {
        i64::from(self.coords.get(1).copied().unwrap_or(0) as i32)
    }
    pub fn level(&self) -> u32 {
        self.coords.last().copied().unwrap_or(0)
    }
    /// Indices along the non-XY dimensions.
    pub fn extra(&self) -> &[u32] {
        let n = self.coords.len();
        if n >= 3 { &self.coords[2..n - 1] } else { &[] }
    }
}

/// A parsed ETS file.
#[derive(Debug)]
pub struct EtsFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub header: EtsHeader,
    pub tiles: Vec<Tile>,
    /// Why (part of) the tile table could not be read.
    pub table_problem: Option<String>,
    file: Option<SourceFile>,
    fs: Fs,
}

/// Does `head` look like an ETS file?
pub fn looks_like_ets(head: &[u8]) -> bool {
    head.len() >= 68 && &head[..4] == SIS_MAGIC && &head[64..68] == ETS_MAGIC
}

impl EtsFile {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut head = vec![0u8; 64];
        f.read_exact(&mut head).map_err(|_| {
            Error::corrupt(
                FORMAT_ID,
                format!("{} is shorter than the 64-byte ETS header", path.display()),
            )
        })?;
        if &head[..4] != SIS_MAGIC {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("{} does not start with SIS", path.display()),
            ));
        }
        let coord_count = le_u32(&head, 12).unwrap_or(0);
        let ets_offset = le_u64(&head, 16).unwrap_or(0);
        let ets_size = le_u32(&head, 24).unwrap_or(0);
        let table_offset = le_u64(&head, 32).unwrap_or(0);
        let tile_count = le_u32(&head, 40).unwrap_or(0);
        if !(3..=16).contains(&coord_count) || !(192..=4096).contains(&ets_size) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                12,
                format!(
                    "{}: implausible ETS header ({coord_count} coordinates, second header of {ets_size} bytes)",
                    path.display()
                ),
            ));
        }
        if ets_offset.saturating_add(u64::from(ets_size)) > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                ets_offset,
                format!(
                    "{}: ETS header runs past the end of the file (truncated?)",
                    path.display()
                ),
            ));
        }
        let mut e = vec![0u8; ets_size as usize];
        f.seek(SeekFrom::Start(ets_offset))
            .and_then(|_| f.read_exact(&mut e))
            .map_err(|err| Error::io(path, err))?;
        if &e[..4] != ETS_MAGIC {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                ets_offset,
                format!("{}: second header does not start with ETS", path.display()),
            ));
        }
        let n_sizes = (le_u32(&e, 184).unwrap_or(0) as usize).min((e.len() - 188) / 4);
        let header = EtsHeader {
            coord_count,
            ets_offset,
            ets_size,
            table_offset,
            tile_count,
            version: le_u32(&e, 4).unwrap_or(0),
            sample_type: le_u32(&e, 8).unwrap_or(0),
            samples_per_pixel: le_u32(&e, 12).unwrap_or(0),
            color_space: le_u32(&e, 16).unwrap_or(0),
            compression: EtsCompression::from_code(le_u32(&e, 20).unwrap_or(0)),
            quality: le_u32(&e, 24).unwrap_or(0),
            tile_width: le_u32(&e, 28).unwrap_or(0),
            tile_height: le_u32(&e, 32).unwrap_or(0),
            tile_depth: le_u32(&e, 36).unwrap_or(0),
            background: le_u32(&e, 108).unwrap_or(0),
            sizes: (0..n_sizes)
                .map(|i| le_u32(&e, 188 + 4 * i).unwrap_or(0))
                .collect(),
        };
        let mut ets = EtsFile {
            path: path.to_path_buf(),
            file_len,
            header,
            tiles: Vec::new(),
            table_problem: None,
            file: Some(f),
            fs: fs.clone(),
        };
        ets.load_table();
        Ok(ets)
    }

    fn load_table(&mut self) {
        let h = &self.header;
        if h.tile_count > MAX_TILES {
            self.table_problem = Some(format!("{} tiles is implausibly many", h.tile_count));
            return;
        }
        let esz = h.entry_size();
        let want = esz * u64::from(h.tile_count);
        let avail = self.file_len.saturating_sub(h.table_offset);
        if h.table_offset > self.file_len {
            self.table_problem = Some(format!(
                "tile table offset {} is beyond the end of the file ({} bytes; truncated?)",
                h.table_offset, self.file_len
            ));
            return;
        }
        let n = if want > avail {
            self.table_problem = Some(format!(
                "tile table needs {want} bytes, {avail} remain (file truncated?)"
            ));
            avail / esz
        } else {
            u64::from(h.tile_count)
        };
        let Ok(bytes) = usize::try_from(n * esz) else {
            return;
        };
        let mut buf = vec![0u8; bytes];
        let Some(f) = self.file.as_mut() else { return };
        if f.seek(SeekFrom::Start(h.table_offset))
            .and_then(|_| f.read_exact(&mut buf))
            .is_err()
        {
            self.table_problem = Some("tile table cannot be read".into());
            return;
        }
        let nc = h.coord_count as usize;
        let esz = esz as usize;
        self.tiles = buf
            .chunks_exact(esz)
            .map(|c| Tile {
                coords: (0..nc).map(|k| le_u32(c, 4 + 4 * k).unwrap_or(0)).collect(),
                offset: le_u64(c, 4 + 4 * nc).unwrap_or(0),
                len: le_u32(c, 12 + 4 * nc).unwrap_or(0),
            })
            .collect();
    }

    /// Number of pyramid levels (1 + largest level index).
    pub fn levels(&self) -> u32 {
        self.tiles
            .iter()
            .map(Tile::level)
            .max()
            .map_or(1, |m| m + 1)
    }

    /// Stored bytes of one tile.
    pub fn read_tile_bytes(&mut self, t: &Tile) -> Result<Vec<u8>> {
        if t.offset.saturating_add(u64::from(t.len)) > self.file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                t.offset,
                format!(
                    "{}: tile runs past the end of the file (truncated?)",
                    self.path.display()
                ),
            ));
        }
        let path = self.path.clone();
        if self.file.is_none() {
            self.file = Some(self.fs.open(&path).map_err(|e| Error::io(&path, e))?);
        }
        let f = self.file.as_mut().expect("opened");
        let mut buf = vec![0u8; t.len as usize];
        f.seek(SeekFrom::Start(t.offset))
            .and_then(|_| f.read_exact(&mut buf))
            .map_err(|e| Error::io(&path, e))?;
        Ok(buf)
    }

    /// Decode one tile to `tile_width × tile_height × samples_per_pixel` little-endian samples.
    pub fn decode_tile(&mut self, t: &Tile) -> Result<Vec<u8>> {
        let raw = self.read_tile_bytes(t)?;
        self.decode_tile_bytes(t, raw)
    }

    /// The stored (compressed) bytes of tile `t`.
    pub fn tile_bytes(&mut self, t: &Tile) -> Result<Vec<u8>> {
        self.read_tile_bytes(t)
    }

    /// Decode the stored bytes `raw` of tile `t` (see [`EtsFile::decode_tile`]); needs no file
    /// access, so the tiles of one region are decoded on several threads.
    pub fn decode_tile_bytes(&self, t: &Tile, raw: Vec<u8>) -> Result<Vec<u8>> {
        let h = &self.header;
        let pt = h.pixel_type().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("ETS sample type {}", h.sample_type),
                "Only 8-bit (2) and 16-bit (4) ETS samples are known.",
            )
        })?;
        let bps = pt.bytes_per_sample();
        let (tw, th, spp) = (
            h.tile_width as usize,
            h.tile_height as usize,
            h.samples_per_pixel.max(1) as usize,
        );
        let expected = tw
            .checked_mul(th)
            .and_then(|v| v.checked_mul(spp))
            .and_then(|v| v.checked_mul(bps))
            .filter(|v| *v <= 1 << 30)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "tile size overflows"))?;
        let decoded = match h.compression {
            EtsCompression::Raw => {
                if raw.len() < expected {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        t.offset,
                        format!("raw tile holds {} bytes, {expected} expected", raw.len()),
                    ));
                }
                let mut v = raw;
                v.truncate(expected);
                return Ok(v);
            }
            // Bound the frame by the tile size (with slack for MCU padding) before decoding.
            EtsCompression::Jpeg | EtsCompression::JpegLossless => openreadout_codecs::jpeg_decode_limited(
                &raw,
                expected.saturating_mul(4).max(1 << 20),
            ),
            EtsCompression::Jpeg2000 => openreadout_codecs::jpeg2000_decode(&raw),
            EtsCompression::Other(c) => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("ETS compression code {c}"),
                    "Only raw (0), JPEG (2), JPEG 2000 (3) and lossless JPEG (5) ETS tiles are decoded.",
                ));
            }
        }
        .map_err(|e| {
            Error::corrupt_at(
                FORMAT_ID,
                t.offset,
                format!("{} tile does not decode: {e}", h.compression.name()),
            )
        })?;
        if decoded.channels as usize != spp || decoded.bits_per_sample as usize != 8 * bps {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "{} tile with {} channel(s) of {} bits in a {spp}-sample {}-bit image",
                    h.compression.name(),
                    decoded.channels,
                    decoded.bits_per_sample,
                    8 * bps
                ),
                "The tile codestream disagrees with the ETS header; please report the file.",
            ));
        }
        let (dw, dh) = (decoded.width as usize, decoded.height as usize);
        if dw == tw && dh == th {
            let mut v = decoded.data;
            v.truncate(expected);
            return Ok(v);
        }
        // A codestream smaller or larger than the tile: copy the overlap.
        let mut out = vec![0u8; expected];
        let px = spp * bps;
        for y in 0..th.min(dh) {
            let n = tw.min(dw) * px;
            out[y * tw * px..y * tw * px + n]
                .copy_from_slice(&decoded.data[y * dw * px..y * dw * px + n]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ETS: 2×2 raw u16 tiles of a 3×3 image, one extra dimension of 2, one level.
    pub(crate) fn write_small_ets(path: &Path) {
        let mut d = vec![0u8; 64];
        d[..4].copy_from_slice(SIS_MAGIC);
        let coord_count = 4u32; // x, y, c, level
        d[4..8].copy_from_slice(&64u32.to_le_bytes());
        d[8..12].copy_from_slice(&3u32.to_le_bytes());
        d[12..16].copy_from_slice(&coord_count.to_le_bytes());
        d[16..24].copy_from_slice(&64u64.to_le_bytes());
        d[24..28].copy_from_slice(&228u32.to_le_bytes());
        let mut e = vec![0u8; 228];
        e[..4].copy_from_slice(ETS_MAGIC);
        for (i, w) in [0x0003_0006u32, 4, 1, 1, 0, 100, 2, 2, 1]
            .iter()
            .enumerate()
        {
            e[4 + 4 * i..8 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        e[108..112].copy_from_slice(&0xFFFFu32.to_le_bytes());
        for (i, w) in [3u32, 3, 3, 2].iter().enumerate() {
            e[184 + 4 * i..188 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        d.extend(e);
        let mut table = Vec::new();
        let mut v = 0u16;
        // tiles for c = 0 and c = 1; the (1, 1) tile of c = 1 is missing
        for c in 0..2u32 {
            for (tx, ty) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
                if c == 1 && (tx, ty) == (1, 1) {
                    continue;
                }
                let off = d.len() as u64;
                for _ in 0..4 {
                    d.extend_from_slice(&v.to_le_bytes());
                    v += 1;
                }
                table.extend_from_slice(&coord_count.to_le_bytes());
                for w in [tx, ty, c, 0] {
                    table.extend_from_slice(&w.to_le_bytes());
                }
                table.extend_from_slice(&off.to_le_bytes());
                table.extend_from_slice(&8u32.to_le_bytes());
                table.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        let table_off = d.len() as u64;
        let n = (table.len() / 36) as u32;
        d.extend(table);
        d[32..40].copy_from_slice(&table_off.to_le_bytes());
        d[40..44].copy_from_slice(&n.to_le_bytes());
        std::fs::write(path, d).unwrap();
    }

    #[test]
    fn reads_header_and_table() {
        let dir = std::env::temp_dir().join(format!("vsi-ets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("frame_t.ets");
        write_small_ets(&p);
        let mut e = EtsFile::open(&p).unwrap();
        assert!(e.table_problem.is_none());
        assert_eq!(e.header.sizes, vec![3, 3, 2]);
        assert_eq!(e.header.pixel_type(), Some(PixelType::Uint16));
        assert_eq!(e.tiles.len(), 7);
        assert_eq!(e.tiles[4].extra(), &[1]);
        assert_eq!(e.levels(), 1);
        let t = e.tiles[1].clone();
        let px = e.decode_tile(&t).unwrap();
        assert_eq!(px, [4, 0, 5, 0, 6, 0, 7, 0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compression_codes() {
        assert_eq!(EtsCompression::from_code(5), EtsCompression::JpegLossless);
        assert_eq!(EtsCompression::JpegLossless.name(), "jpeg-lossless");
        assert_eq!(EtsCompression::from_code(8), EtsCompression::Other(8));
    }
}
