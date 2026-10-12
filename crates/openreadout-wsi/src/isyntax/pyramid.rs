//! The wavelet pyramid of an iSyntax slide: geometry from the general header, the code blocks
//! from the block header table, their place in the seek table, and which tiles exist.
//! `docs/formats/philips-isyntax.md` § Pyramid.

use std::collections::HashMap;

use openreadout_core::{Error, Result};

use super::FORMAT_ID;
use super::header::{Header, ScannedImage};

/// Padding added on each side of level 0 per scale: the image is padded by `3·2^L − 3`.
pub const PER_LEVEL_PADDING: i64 = 3;
/// Bytes of one entry of the block header table (partial entries, offsets in the seek table).
pub const BLOCK_ENTRY: usize = 48;
/// Bytes of one seek-table entry.
pub const SEEK_ENTRY: u64 = 40;
/// Most scales accepted.
const MAX_LEVELS: i64 = 16;
/// Largest code block side accepted.
const MAX_BLOCK_SIDE: i64 = 1024;
/// Most code blocks accepted.
const MAX_BLOCKS: usize = 1 << 26;

/// One code block of the block header table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeBlock {
    /// Stored x and y coordinates (padded image, with the image offset).
    pub x: u32,
    /// See `x`.
    pub y: u32,
    /// Colour component: 0 Y, 1 Co, 2 Cg.
    pub color: u8,
    /// Scale (0 = full resolution).
    pub scale: u8,
    /// `true` for an LL block (coefficient 0), `false` for HL, LH and HH together.
    pub ll: bool,
    /// Block header template.
    pub template: u32,
    /// Tile column and row on the grid of its scale.
    pub tile_x: u32,
    /// See `tile_x`.
    pub tile_y: u32,
    /// Index of its seek-table entry.
    pub block_id: u64,
}

/// Geometry and code blocks of the slide image.
#[derive(Debug, Clone)]
pub struct Pyramid {
    /// Code block width and height (coefficients); tiles are twice as large in pixels.
    pub block_w: u32,
    /// See `block_w`.
    pub block_h: u32,
    /// Number of scales.
    pub levels: u32,
    /// Start of the x and y ranges: the coordinates of the padded image's first pixel.
    pub offset: (i64, i64),
    /// Size of the padded image (level 0).
    pub padded: (u64, u64),
    /// Padding on each side of level 0: `3·2^levels − 3`.
    pub padding: u64,
    /// Image size at level 0 (padded size minus twice the padding).
    pub size: (u64, u64),
    /// Tile grid of scale 0 (columns, rows); scale s has `grid >> s`.
    pub grid: (u64, u64),
    /// Micrometres per pixel along x and y (dimension scale factors), when the unit is µm.
    pub mpp: (Option<f64>, Option<f64>),
    /// The code blocks, in table order.
    pub blocks: Vec<CodeBlock>,
    /// `UFS_IMAGE_BLOCK_COMPRESSION_METHOD` of the templates.
    pub compression: String,
    index: HashMap<(u8, bool, u8, u32, u32), u32>,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, detail.into())
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// First padded-image coordinate of the tile grid of scale `s` for high-pass blocks.
fn first_coef(s: u32) -> i64 {
    (PER_LEVEL_PADDING << s) - (PER_LEVEL_PADDING - 1)
}

impl Pyramid {
    /// Tile columns and rows of scale `s`.
    pub fn tiles(&self, s: u32) -> (u64, u64) {
        (self.grid.0 >> s, self.grid.1 >> s)
    }

    /// Tile count of scale `s`.
    pub fn tile_count(&self, s: u32) -> u64 {
        (self.grid.0 * self.grid.1) >> (2 * s)
    }

    /// Image size at scale `s` (level-0 size shifted right).
    pub fn level_size(&self, s: u32) -> (u64, u64) {
        (self.size.0 >> s, self.size.1 >> s)
    }

    /// Offset of a level's first pixel on its tile grid: the level-0 padding shifted right.
    pub fn level_origin(&self, s: u32) -> u64 {
        self.padding >> s
    }

    /// Tile width and height in pixels.
    pub fn tile_size(&self) -> (u64, u64) {
        (2 * u64::from(self.block_w), 2 * u64::from(self.block_h))
    }

    /// The code block of colour `color` holding the LL (`ll`) or high-pass coefficients of
    /// tile (`x`, `y`) of scale `s`.
    pub fn block(&self, s: u32, ll: bool, color: u8, x: u32, y: u32) -> Option<u32> {
        let s = u8::try_from(s).ok()?;
        self.index.get(&(s, ll, color, x, y)).copied()
    }

    /// Whether tile (`x`, `y`) of scale `s` exists (has code blocks).
    pub fn exists(&self, s: u32, x: i64, y: i64) -> bool {
        let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
            return false;
        };
        self.block(s, false, 0, x, y).is_some() || self.block(s, true, 0, x, y).is_some()
    }

    /// Build the pyramid of the slide image.
    pub fn from_header(h: &Header, wsi: &ScannedImage) -> Result<Pyramid> {
        if let Some((major, minor)) = h.data_model()
            && major >= 100
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("iSyntax data model {major}.{minor} (cluster headers)"),
                "Only data model 5 files (PIM_DP_UFS_INTERFACE_VERSION 5.x) are read: no public file of the newer layout has been available to validate it. Export the slide to OME-TIFF or DICOM with the scanner software, or send a sample (see `openreadout report`).",
            ));
        }
        if wsi.has_cluster_table {
            return Err(Error::unsupported(
                FORMAT_ID,
                "iSyntax cluster header table",
                "Files with UFS_IMAGE_CLUSTER_HEADER_TABLE (data model 100 and later) are not read yet.",
            ));
        }
        let first = wsi
            .templates
            .first()
            .ok_or_else(|| bad("no block header template"))?;
        let side = |t: &super::header::BlockTemplate, k: usize| -> Result<i64> {
            let n = t
                .ranges
                .get(k)
                .ok_or_else(|| bad("a block header template has fewer than 2 dimension ranges"))?
                .count();
            if !(4..=MAX_BLOCK_SIDE).contains(&n) || n % 4 != 0 {
                return Err(bad(format!(
                    "code block side {n} (expected a multiple of 4 up to {MAX_BLOCK_SIDE})"
                )));
            }
            Ok(n)
        };
        let (bw, bh) = (side(first, 0)?, side(first, 1)?);
        let mut compression = None;
        for t in &wsi.templates {
            if (side(t, 0)?, side(t, 1)?) != (bw, bh) {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "code blocks of different sizes",
                    "Every block header template of the corpus file has the same size; mixed sizes are not read.",
                ));
            }
            let c = t.compression.clone().unwrap_or_default();
            match compression.as_ref() {
                None => compression = Some(c),
                Some(prev) if *prev != c => {
                    return Err(bad(
                        "block header templates name different compression methods",
                    ));
                }
                Some(_) => {}
            }
        }
        let compression = compression.unwrap_or_default();
        if compression != "16" {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("iSyntax block compression method {compression:?}"),
                "Only compression method 16 (the first iSyntax compressor) is read; method 19 has no public sample file yet.",
            ));
        }
        if wsi.ranges.len() < 4 {
            return Err(bad("the general header has fewer than 4 dimension ranges"));
        }
        let (rx, ry, rs) = (wsi.ranges[0], wsi.ranges[1], wsi.ranges[3]);
        let levels = rs.count();
        if !(1..=MAX_LEVELS).contains(&levels) {
            return Err(bad(format!("{levels} scales (expected 1 to {MAX_LEVELS})")));
        }
        let (pw, ph) = (rx.count(), ry.count());
        let padding = (PER_LEVEL_PADDING << levels) - PER_LEVEL_PADDING;
        if pw <= 2 * padding || ph <= 2 * padding || pw > 1 << 31 || ph > 1 << 31 {
            return Err(bad(format!(
                "padded image {pw} x {ph} is not larger than twice the padding ({padding})"
            )));
        }
        let span_x = bw << levels;
        let span_y = bh << levels;
        let grid = (
            (((pw + span_x - 1) / span_x) << (levels - 1)) as u64,
            (((ph + span_y - 1) / span_y) << (levels - 1)) as u64,
        );
        let unit_um = |k: usize| {
            wsi.dimensions.get(k).and_then(|d| {
                let um = d.unit.as_deref().is_some_and(|u| {
                    u.eq_ignore_ascii_case("MicroMeter") || u.eq_ignore_ascii_case("um")
                });
                d.scale_factor.filter(|v| um && *v > 0.0 && v.is_finite())
            })
        };
        let mut p = Pyramid {
            block_w: bw as u32,
            block_h: bh as u32,
            levels: levels as u32,
            offset: (rx.start, ry.start),
            padded: (pw as u64, ph as u64),
            padding: padding as u64,
            size: ((pw - 2 * padding) as u64, (ph - 2 * padding) as u64),
            grid,
            mpp: (unit_um(0), unit_um(1)),
            blocks: Vec::new(),
            compression,
            index: HashMap::new(),
        };
        let table = wsi
            .block_table
            .as_deref()
            .ok_or_else(|| bad("the slide image has no block header table"))?;
        p.read_blocks(table, wsi.templates.len())?;
        Ok(p)
    }

    #[allow(clippy::many_single_char_names)]
    fn read_blocks(&mut self, table: &[u8], templates: usize) -> Result<()> {
        if table.len() < 4 {
            return Err(bad("block header table shorter than its length field"));
        }
        let declared = le_u32(table, 0) as usize;
        let body = table.get(4..4 + declared).ok_or_else(|| {
            bad(format!(
                "block header table declares {declared} bytes, holds {}",
                table.len() - 4
            ))
        })?;
        if body.len() >= 8 && le_u32(body, 4) == 72 {
            return Err(Error::unsupported(
                FORMAT_ID,
                "block header table with offsets (80-byte entries)",
                "Only tables whose offsets are in the seek table (48-byte entries) are read; this layout has no public sample.",
            ));
        }
        if !declared.is_multiple_of(BLOCK_ENTRY) {
            return Err(bad(format!(
                "block header table of {declared} bytes is not a whole number of {BLOCK_ENTRY}-byte entries"
            )));
        }
        let n = declared / BLOCK_ENTRY;
        if n > MAX_BLOCKS {
            return Err(bad(format!("{n} code blocks (more than {MAX_BLOCKS})")));
        }
        let levels = self.levels;
        let top = levels - 1;
        let total_tiles: u64 =
            (0..levels).map(|s| self.tile_count(s)).sum::<u64>() + self.tile_count(top);
        let (tw, th) = self.tile_size();
        self.blocks.reserve(n);
        for k in 0..n {
            let e = &body[k * BLOCK_ENTRY..(k + 1) * BLOCK_ENTRY];
            if le_u16(e, 0) != 0xFFFE || le_u16(e, 2) != 0xE000 || le_u32(e, 4) != 40 {
                return Err(bad(format!(
                    "block header table entry {k} does not start with a 40-byte sequence item"
                )));
            }
            let (x, y) = (le_u32(e, 16), le_u32(e, 20));
            let (color, scale, coef) = (le_u32(e, 24), le_u32(e, 28), le_u32(e, 32));
            let template = le_u32(e, 44);
            if color > 2 || scale >= levels || coef > 1 || template as usize >= templates {
                return Err(bad(format!(
                    "block header table entry {k}: colour {color}, scale {scale}, coefficient {coef}, template {template} out of range"
                )));
            }
            let ll = coef == 0;
            if ll && scale != top {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("an LL code block at scale {scale} (below the top scale {top})"),
                    "LL coefficients are known only at the top scale.",
                ));
            }
            let mut off = first_coef(scale);
            if ll {
                off += 1 << scale;
            }
            let ax = i64::from(x) - self.offset.0 - off;
            let ay = i64::from(y) - self.offset.1 - off;
            let (cols, rows) = self.tiles(scale);
            let tx = ax.div_euclid((tw as i64) << scale);
            let ty = ay.div_euclid((th as i64) << scale);
            if ax < 0 || ay < 0 || tx as u64 >= cols || ty as u64 >= rows {
                return Err(bad(format!(
                    "block header table entry {k}: tile ({tx}, {ty}) of scale {scale} lies outside its {cols} x {rows} grid"
                )));
            }
            let mut id: u64 = (0..if ll { scale + 1 } else { scale })
                .map(|s| self.tile_count(s))
                .sum();
            id += ty as u64 * cols + tx as u64;
            id += u64::from(color) * total_tiles;
            let b = CodeBlock {
                x,
                y,
                color: color as u8,
                scale: scale as u8,
                ll,
                template,
                tile_x: tx as u32,
                tile_y: ty as u32,
                block_id: id,
            };
            if self
                .index
                .insert((b.scale, ll, b.color, b.tile_x, b.tile_y), k as u32)
                .is_some()
            {
                return Err(bad(format!(
                    "two code blocks for tile ({tx}, {ty}) of scale {scale}, colour {color}"
                )));
            }
            self.blocks.push(b);
        }
        Ok(())
    }

    /// Number of tiles with code blocks at scale `s`.
    pub fn existing_tiles(&self, s: u32) -> usize {
        self.blocks
            .iter()
            .filter(|b| u32::from(b.scale) == s && b.color == 0 && !b.ll)
            .count()
    }
}

/// Where each code block's data lies: the seek table that follows the header.
pub fn parse_seek_table(
    table: &[u8],
    p: &Pyramid,
    file_len: u64,
    data_start: u64,
) -> Result<Vec<(u64, u64)>> {
    let mut out = Vec::with_capacity(p.blocks.len());
    for (k, b) in p.blocks.iter().enumerate() {
        let at = usize::try_from(b.block_id * SEEK_ENTRY)
            .ok()
            .and_then(|a| table.get(a..a + SEEK_ENTRY as usize))
            .ok_or_else(|| {
                bad(format!(
                    "code block {k} (seek entry {}) lies past the seek table",
                    b.block_id
                ))
            })?;
        if le_u16(at, 0) != 0xFFFE || le_u16(at, 2) != 0xE000 {
            return Err(bad(format!(
                "seek-table entry {} is not a sequence item",
                b.block_id
            )));
        }
        let offset = u64::from_le_bytes(at[16..24].try_into().unwrap_or([0; 8]));
        let size = u64::from_le_bytes(at[32..40].try_into().unwrap_or([0; 8]));
        if size > 0 && (offset < data_start || offset.saturating_add(size) > file_len) {
            return Err(bad(format!(
                "code block {k}: {size} bytes at {offset} lie outside the data (file of {file_len} bytes)"
            )));
        }
        if size > 64 << 20 {
            return Err(bad(format!("code block {k} of {size} bytes")));
        }
        out.push((offset, size));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isyntax::header::{BlockTemplate, DimRange, ScannedImage};

    fn range(start: i64, step: i64, end: i64) -> DimRange {
        DimRange { start, step, end }
    }

    #[allow(clippy::many_single_char_names)]
    fn entry(x: u32, y: u32, c: u32, s: u32, coef: u32) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend_from_slice(&0xFFFEu16.to_le_bytes());
        e.extend_from_slice(&0xE000u16.to_le_bytes());
        e.extend_from_slice(&40u32.to_le_bytes());
        e.extend_from_slice(&[0x1D, 0x30, 0x0E, 0x20, 20, 0, 0, 0]);
        for v in [x, y, c, s, coef] {
            e.extend_from_slice(&v.to_le_bytes());
        }
        e.extend_from_slice(&[0x1D, 0x30, 0x12, 0x20, 4, 0, 0, 0]);
        e.extend_from_slice(&0u32.to_le_bytes());
        e
    }

    #[test]
    fn geometry_and_block_ids_follow_the_padding_rules() {
        // Two scales of 8 x 8 blocks: padding 3·4 − 3 = 9, grid ceil(64/32) << 1 = 4 tiles.
        let mut wsi = ScannedImage {
            kind: "WSI".into(),
            templates: vec![BlockTemplate {
                ranges: vec![range(0, 8, 56), range(0, 8, 56)],
                compression: Some("16".into()),
            }],
            ranges: vec![
                range(100, 1, 163),
                range(200, 1, 263),
                range(0, 1, 2),
                range(0, 1, 1),
            ],
            ..ScannedImage::default()
        };
        // Tile (1, 0) of scale 0, colour 0: x = 100 + 1 + 16, y = 200 + 1.
        // LL of tile (0, 0) at the top scale 1: offset first_coef(1) + 2 = 4 + 2.
        let mut table = Vec::new();
        table.extend(entry(117, 201, 0, 0, 1));
        table.extend(entry(106, 206, 0, 1, 0));
        let mut b64 = (table.len() as u32).to_le_bytes().to_vec();
        b64.extend(table);
        wsi.block_table = Some(b64);
        let h = Header::default();
        let p = Pyramid::from_header(&h, &wsi).unwrap();
        assert_eq!((p.block_w, p.levels, p.padding), (8, 2, 9));
        assert_eq!(p.size, (64 - 18, 64 - 18));
        assert_eq!(p.grid, (4, 4));
        assert_eq!(
            (p.blocks[0].tile_x, p.blocks[0].tile_y, p.blocks[0].block_id),
            (1, 0, 1)
        );
        // LL ids follow every high-pass tile of scales 0 and 1: 16 + 4.
        assert_eq!(p.blocks[1].block_id, 20);
        assert!(p.exists(0, 1, 0) && p.exists(1, 0, 0) && !p.exists(0, 0, 0));
        assert_eq!(p.level_origin(1), 4);

        // Compression 19 and data model 100 are refused with exit 6.
        let mut w19 = wsi.clone();
        w19.templates[0].compression = Some("19".into());
        assert_eq!(Pyramid::from_header(&h, &w19).unwrap_err().exit_code(), 6);
        let mut h100 = Header::default();
        h100.attributes
            .insert("PIM_DP_UFS_INTERFACE_VERSION".into(), "100.5".into());
        assert_eq!(
            Pyramid::from_header(&h100, &wsi).unwrap_err().exit_code(),
            6
        );
        // A block outside its grid is corrupt.
        let mut wbad = wsi.clone();
        let mut t = entry(100_000, 201, 0, 0, 1);
        let mut b = (t.len() as u32).to_le_bytes().to_vec();
        b.append(&mut t);
        wbad.block_table = Some(b);
        assert_eq!(Pyramid::from_header(&h, &wbad).unwrap_err().exit_code(), 4);
    }
}
