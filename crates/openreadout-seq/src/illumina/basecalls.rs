//! Base calls under `Data/Intensities/BaseCalls`: per-cycle BCL files (one per tile, `.bcl` or
//! `.bcl.gz`) or CBCL files (one per lane and surface, every tile a gzip block), the per-tile
//! filter files (which clusters passed the chastity filter) and the cluster locations
//! (`.locs` per tile, or one `s.locs` for a patterned flow cell).
//!
//! A base call is one byte here, as in a BCL file: bits 0–1 the base (A, C, G, T), bits 2–7 the
//! quality score; 0 is a no-call (written `N` with quality 2, as the conversion tools do). CBCL
//! nibbles are widened to this form through the file's quality bins.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use super::FORMAT_ID;

/// Largest uncompressed tile block or BCL file read (bytes).
const MAX_TILE_BYTES: u64 = 1 << 30;
/// Largest CBCL header read.
const MAX_CBCL_HEADER: u32 = 16 << 20;

/// How the base calls of a run are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStorage {
    /// One `.bcl` (or `.bcl.gz`) per cycle and tile.
    Bcl,
    /// One `.cbcl` per cycle, lane and surface.
    Cbcl,
}

/// A tile block inside a CBCL file.
#[derive(Debug, Clone, Copy)]
pub struct CbclTile {
    /// Tile number.
    pub tile: u32,
    /// Clusters in the block.
    pub clusters: u32,
    /// Uncompressed bytes.
    pub raw_len: u32,
    /// Compressed bytes.
    pub stored_len: u32,
    /// Offset of the gzip block in the file.
    pub offset: u64,
}

/// A CBCL header.
#[derive(Debug, Clone)]
pub struct CbclHeader {
    /// Format version.
    pub version: u16,
    /// Header length in bytes.
    pub header_len: u32,
    /// Bits per base call.
    pub bits_per_base: u8,
    /// Bits per quality score.
    pub bits_per_quality: u8,
    /// Quality score of each bin index.
    pub bins: Vec<u8>,
    /// Tile blocks in file order.
    pub tiles: Vec<CbclTile>,
    /// True when clusters that failed the filter were left out of the blocks.
    pub pf_excluded: bool,
}

/// Parse a CBCL header from the first bytes of the file.
pub fn parse_cbcl_header(b: &[u8], name: &str) -> Result<CbclHeader> {
    let bad = |what: &str| Error::corrupt(FORMAT_ID, format!("{name}: {what}"));
    let u32_at = |at: usize| -> Option<u32> {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let version = b
        .get(0..2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("too short for a CBCL header"))?;
    let header_len = u32_at(2).ok_or_else(|| bad("too short for a CBCL header"))?;
    let (bits_per_base, bits_per_quality) = match (b.get(6), b.get(7)) {
        (Some(x), Some(y)) => (*x, *y),
        _ => return Err(bad("too short for a CBCL header")),
    };
    if bits_per_base != 2 || bits_per_quality != 2 {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("{name}: CBCL with {bits_per_base} bits per base and {bits_per_quality} per quality"),
            "Only CBCL files with 2-bit bases and 2-bit quality bins (every instrument seen so far) are decoded; the InterOp metrics are still read.",
        ));
    }
    let nbins = u32_at(8).ok_or_else(|| bad("truncated bin table"))? as usize;
    if nbins > 16 {
        return Err(bad("more than 16 quality bins in a 2-bit CBCL"));
    }
    let mut bins = vec![0u8; 4];
    let mut at = 12usize;
    for _ in 0..nbins {
        let from = u32_at(at).ok_or_else(|| bad("truncated bin table"))?;
        let to = u32_at(at + 4).ok_or_else(|| bad("truncated bin table"))?;
        if let Some(slot) = bins.get_mut(from as usize) {
            *slot = to.min(93) as u8;
        }
        at += 8;
    }
    let ntiles = u32_at(at).ok_or_else(|| bad("truncated tile table"))? as usize;
    at += 4;
    if ntiles > b.len() / 16 + 1 {
        return Err(bad("tile count larger than the header"));
    }
    let mut tiles = Vec::with_capacity(ntiles);
    let mut offset = u64::from(header_len);
    for _ in 0..ntiles {
        let (Some(tile), Some(clusters), Some(raw_len), Some(stored_len)) =
            (u32_at(at), u32_at(at + 4), u32_at(at + 8), u32_at(at + 12))
        else {
            return Err(bad("truncated tile table"));
        };
        tiles.push(CbclTile {
            tile,
            clusters,
            raw_len,
            stored_len,
            offset,
        });
        offset = offset.saturating_add(u64::from(stored_len));
        at += 16;
    }
    let pf_excluded = b.get(at).copied().unwrap_or(0) == 1;
    Ok(CbclHeader {
        version,
        header_len,
        bits_per_base,
        bits_per_quality,
        bins,
        tiles,
        pf_excluded,
    })
}

/// The base-call files of a run folder, found by listing directories (cheap: cycle
/// directories and one cycle's files, not every file).
#[derive(Debug, Clone)]
pub struct BaseCalls {
    /// `Data/Intensities/BaseCalls`.
    pub dir: PathBuf,
    /// BCL or CBCL.
    pub storage: CallStorage,
    /// Lane → cycles that have a `C<n>.1` directory.
    pub cycles: BTreeMap<u16, Vec<u32>>,
    /// Lane → tiles with a filter file.
    pub tiles: BTreeMap<u16, Vec<u32>>,
}

fn lane_of(name: &str) -> Option<u16> {
    name.strip_prefix('L')?.parse().ok()
}

fn cycle_of(name: &str) -> Option<u32> {
    name.strip_prefix('C')?.strip_suffix(".1")?.parse().ok()
}

/// `s_<lane>_<tile>.<ext>` → tile.
fn tile_of(name: &str, ext: &str) -> Option<u32> {
    let stem = name.strip_suffix(ext)?;
    let mut parts = stem.strip_prefix("s_")?.split('_');
    let _lane = parts.next()?;
    parts.next()?.parse().ok()
}

fn list(fs: &Fs, dir: &Path) -> Vec<String> {
    fs.read_dir(dir)
        .map(|rd| {
            rd.filter_map(std::result::Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

impl BaseCalls {
    /// Find the base calls of the run folder at `root`; `None` when it has none.
    pub fn discover(fs: &Fs, root: &Path) -> Option<BaseCalls> {
        let dir = root.join("Data").join("Intensities").join("BaseCalls");
        if !fs.is_dir(&dir) {
            return None;
        }
        let mut cycles = BTreeMap::new();
        let mut tiles = BTreeMap::new();
        let mut storage = None;
        for lane_name in list(fs, &dir) {
            let Some(lane) = lane_of(&lane_name) else {
                continue;
            };
            let lane_dir = dir.join(&lane_name);
            let mut cs = Vec::new();
            let mut ts = Vec::new();
            for n in list(fs, &lane_dir) {
                if let Some(c) = cycle_of(&n) {
                    cs.push(c);
                } else if let Some(t) = tile_of(&n, ".filter") {
                    ts.push(t);
                }
            }
            if cs.is_empty() {
                continue;
            }
            cs.sort_unstable();
            ts.sort_unstable();
            if storage.is_none() {
                let first = lane_dir.join(format!("C{}.1", cs[0]));
                let names = list(fs, &first);
                if names.iter().any(|n| n.ends_with(".cbcl")) {
                    storage = Some(CallStorage::Cbcl);
                } else if names
                    .iter()
                    .any(|n| n.ends_with(".bcl") || n.ends_with(".bcl.gz"))
                {
                    storage = Some(CallStorage::Bcl);
                }
            }
            cycles.insert(lane, cs);
            tiles.insert(lane, ts);
        }
        Some(BaseCalls {
            dir,
            storage: storage?,
            cycles,
            tiles,
        })
    }

    /// The filter file of a tile.
    pub fn filter_path(&self, lane: u16, tile: u32) -> PathBuf {
        self.dir
            .join(format!("L{lane:03}"))
            .join(format!("s_{lane}_{tile}.filter"))
    }

    /// Read a tile's filter: one bool per cluster, true when it passed.
    pub fn read_filter(&self, fs: &Fs, lane: u16, tile: u32) -> Result<Vec<bool>> {
        let p = self.filter_path(lane, tile);
        let b = read_capped(fs, &p, MAX_TILE_BYTES)?;
        parse_filter(&b, &p.display().to_string())
    }

    /// The locations file of a tile: per-tile `.locs`, else the flow cell's `s.locs`.
    pub fn locs_path(&self, fs: &Fs, lane: u16, tile: u32) -> Option<PathBuf> {
        let intens = self.dir.parent()?;
        let per_tile = intens
            .join(format!("L{lane:03}"))
            .join(format!("s_{lane}_{tile}.locs"));
        if fs.is_file(&per_tile) {
            return Some(per_tile);
        }
        let shared = intens.join("s.locs");
        fs.is_file(&shared).then_some(shared)
    }

    /// Read cluster positions `(x, y)` in pixels; `None` when the run folder has no locations.
    pub fn read_locs(&self, fs: &Fs, lane: u16, tile: u32) -> Result<Option<Vec<(f32, f32)>>> {
        let Some(p) = self.locs_path(fs, lane, tile) else {
            return Ok(None);
        };
        let b = read_capped(fs, &p, MAX_TILE_BYTES.saturating_mul(2))?;
        parse_locs(&b, &p.display().to_string()).map(Some)
    }

    /// The file holding cycle `cycle` of `tile` in `lane`.
    pub fn cycle_file(&self, fs: &Fs, lane: u16, tile: u32, cycle: u32) -> Option<PathBuf> {
        let d = self.dir.join(format!("L{lane:03}")).join(format!("C{cycle}.1"));
        match self.storage {
            CallStorage::Bcl => {
                let p = d.join(format!("s_{lane}_{tile}.bcl"));
                if fs.is_file(&p) {
                    return Some(p);
                }
                let gz = d.join(format!("s_{lane}_{tile}.bcl.gz"));
                fs.is_file(&gz).then_some(gz)
            }
            CallStorage::Cbcl => {
                let surface = surface_of_tile(tile);
                let p = d.join(format!("L{lane:03}_{surface}.cbcl"));
                fs.is_file(&p).then_some(p)
            }
        }
    }

    /// Base calls of clusters `[first, first + count)` of a tile at one cycle, one byte each (see
    /// the module documentation), every cluster of the tile including those that failed the
    /// filter. A CBCL block that left out failed clusters is spread back over all clusters with
    /// `filter` (failed clusters become no-calls).
    pub fn read_cycle(
        &self,
        fs: &Fs,
        lane: u16,
        tile: u32,
        cycle: u32,
        first: u64,
        count: u64,
        filter: &[bool],
    ) -> Result<Vec<u8>> {
        let p = self.cycle_file(fs, lane, tile, cycle).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("no base calls for lane {lane}, tile {tile}, cycle {cycle} (the run folder is incomplete or still being written)"),
            )
        })?;
        let name = p.display().to_string();
        match self.storage {
            CallStorage::Bcl => {
                let f = fs.open(&p).map_err(|e| Error::io(&p, e))?;
                if !name.ends_with(".gz") {
                    let mut head = [0u8; 4];
                    f.read_exact_at(0, &mut head)
                        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                    let n = u64::from(u32::from_le_bytes(head));
                    if first >= n {
                        return Ok(Vec::new());
                    }
                    let want = count.min(n - first).min(MAX_TILE_BYTES);
                    let mut out = vec![0u8; want as usize];
                    f.read_exact_at(4 + first, &mut out).map_err(|e| {
                        Error::corrupt(FORMAT_ID, format!("{name}: truncated BCL ({e})"))
                    })?;
                    return Ok(out);
                }
                let mut reader = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(f));
                let mut head = [0u8; 4];
                reader
                    .read_exact(&mut head)
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                let n = u64::from(u32::from_le_bytes(head));
                if first >= n {
                    return Ok(Vec::new());
                }
                std::io::copy(&mut (&mut reader).take(first), &mut std::io::sink())
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                let want = count.min(n - first).min(MAX_TILE_BYTES);
                let mut out = Vec::with_capacity(want as usize);
                reader
                    .take(want)
                    .read_to_end(&mut out)
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                if (out.len() as u64) < want {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{name}: truncated BCL ({} of {want} clusters)", out.len()),
                    ));
                }
                Ok(out)
            }
            CallStorage::Cbcl => {
                let f = fs.open(&p).map_err(|e| Error::io(&p, e))?;
                let size = f.size().map_err(|e| Error::io(&p, e))?;
                let mut six = [0u8; 6];
                f.read_exact_at(0, &mut six)
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                let hl = u32::from_le_bytes([six[2], six[3], six[4], six[5]]);
                if hl > MAX_CBCL_HEADER || u64::from(hl) > size {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{name}: header length {hl} exceeds the file"),
                    ));
                }
                let mut hb = vec![0u8; hl as usize];
                f.read_exact_at(0, &mut hb)
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                let h = parse_cbcl_header(&hb, &name)?;
                let t = h.tiles.iter().find(|t| t.tile == tile).ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, format!("{name}: no block for tile {tile}"))
                })?;
                if t.offset.saturating_add(u64::from(t.stored_len)) > size {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        t.offset,
                        format!("{name}: tile {tile} block runs past the end of the file (truncated)"),
                    ));
                }
                if u64::from(t.raw_len) > MAX_TILE_BYTES {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{name}: tile {tile} block of {} bytes", t.raw_len),
                    ));
                }
                let mut z = vec![0u8; t.stored_len as usize];
                f.read_exact_at(t.offset, &mut z)
                    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: {e}")))?;
                // Stored clusters in the block: all of the tile's, or only those that passed.
                let stored = u64::from(t.clusters);
                let (skip, take) = if h.pf_excluded {
                    // map [first, first+count) of all clusters to stored (passing) clusters
                    let before = filter.iter().take(first as usize).filter(|x| **x).count() as u64;
                    let inside = filter
                        .iter()
                        .skip(first as usize)
                        .take(count as usize)
                        .filter(|x| **x)
                        .count() as u64;
                    (before, inside)
                } else {
                    (first.min(stored), count.min(stored.saturating_sub(first)))
                };
                let nib = decode_nibbles(&z, skip, take, &name)?;
                let widen = |n: u8| -> u8 {
                    if n == 0 {
                        0
                    } else {
                        let q = h.bins.get((n >> 2) as usize).copied().unwrap_or(0).min(63);
                        (q << 2) | (n & 3)
                    }
                };
                if h.pf_excluded {
                    let mut out = Vec::with_capacity(count as usize);
                    let mut k = 0usize;
                    for &pass in filter.iter().skip(first as usize).take(count as usize) {
                        if pass {
                            out.push(nib.get(k).map_or(0, |n| widen(*n)));
                            k += 1;
                        } else {
                            out.push(0);
                        }
                    }
                    Ok(out)
                } else {
                    Ok(nib.into_iter().map(widen).collect())
                }
            }
        }
    }

    /// The CBCL header of one cycle of a lane and surface (for `check` and the structure view).
    pub fn cbcl_header(&self, fs: &Fs, lane: u16, surface: u32, cycle: u32) -> Result<(PathBuf, CbclHeader, u64)> {
        let p = self
            .dir
            .join(format!("L{lane:03}"))
            .join(format!("C{cycle}.1"))
            .join(format!("L{lane:03}_{surface}.cbcl"));
        let f = fs.open(&p).map_err(|e| Error::io(&p, e))?;
        let size = f.size().map_err(|e| Error::io(&p, e))?;
        let mut hb = vec![0u8; usize::try_from(size.min(64 << 10)).unwrap_or(0)];
        f.read_exact_at(0, &mut hb).map_err(|e| Error::io(&p, e))?;
        let h = parse_cbcl_header(&hb, &p.display().to_string())?;
        Ok((p, h, size))
    }
}

/// The surface digit of a tile number (`2101` → 2, `11101` → 1).
pub fn surface_of_tile(tile: u32) -> u32 {
    let mut t = tile;
    while t >= 10 {
        t /= 10;
    }
    t
}

/// Decode `take` nibbles from nibble `skip` on, streaming through the gzip block.
fn decode_nibbles(z: &[u8], skip: u64, take: u64, name: &str) -> Result<Vec<u8>> {
    let mut d = flate2::read::MultiGzDecoder::new(z);
    let skip_bytes = skip / 2;
    std::io::copy(&mut (&mut d).take(skip_bytes), &mut std::io::sink())
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: tile block: {e}")))?;
    let odd = skip % 2 == 1;
    let need = (take + u64::from(odd)).div_ceil(2);
    let mut raw = Vec::with_capacity(usize::try_from(need).unwrap_or(0));
    d.take(need)
        .read_to_end(&mut raw)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{name}: tile block: {e}")))?;
    let mut out = Vec::with_capacity(take as usize);
    for b in raw {
        out.push(b & 0x0f);
        out.push(b >> 4);
    }
    let start = usize::from(odd);
    let end = (start + take as usize).min(out.len());
    if end - start < take as usize {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{name}: tile block holds {} of {take} clusters", end - start),
        ));
    }
    Ok(out[start..end].to_vec())
}

/// Parse a filter file.
pub fn parse_filter(b: &[u8], name: &str) -> Result<Vec<bool>> {
    if b.len() < 12 {
        return Err(Error::corrupt(FORMAT_ID, format!("{name}: filter file shorter than its header")));
    }
    let word = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (zero, version, n) = (word(0), word(4), word(8) as usize);
    if zero != 0 || version != 3 {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("{name}: filter file version {version}"),
            "Filter files of version 3 (every RTA since 2010) are read.",
        ));
    }
    let body = &b[12..];
    if body.len() < n {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{name}: filter file lists {n} clusters but holds {}", body.len()),
        ));
    }
    Ok(body[..n].iter().map(|x| x & 1 == 1).collect())
}

/// Parse a `.locs` file into `(x, y)` pixel positions.
pub fn parse_locs(b: &[u8], name: &str) -> Result<Vec<(f32, f32)>> {
    if b.len() < 12 {
        return Err(Error::corrupt(FORMAT_ID, format!("{name}: locations file shorter than its header")));
    }
    let n = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    let body = &b[12..];
    if body.len() / 8 < n {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{name}: locations file lists {n} clusters but holds {}", body.len() / 8),
        ));
    }
    Ok(body
        .chunks_exact(8)
        .take(n)
        .map(|c| {
            (
                f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            )
        })
        .collect())
}

/// Read a whole file of at most `max` bytes.
pub fn read_capped(fs: &Fs, p: &Path, max: u64) -> Result<Vec<u8>> {
    let f = fs.open(p).map_err(|e| Error::io(p, e))?;
    let size = f.size().map_err(|e| Error::io(p, e))?;
    if size > max {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("{}: {size} bytes", p.display()),
            "The file is larger than the reader's limit for it; check that it belongs to an Illumina run folder.",
        ));
    }
    let mut b = vec![0u8; size as usize];
    f.read_exact_at(0, &mut b).map_err(|e| Error::io(p, e))?;
    Ok(b)
}

/// The FASTQ cluster coordinate of a location, as the conversion tools write it:
/// `round(10 * position + 1000)`, computed in single precision like the stored position (a
/// position such as 1418.95 lands on .5 in single precision and rounds up).
pub fn fastq_coordinate(pos: f32) -> i64 {
    (pos * 10.0 + 1000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbcl_header_and_garbage() {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&45u32.to_le_bytes());
        b.extend_from_slice(&[2, 2]);
        b.extend_from_slice(&2u32.to_le_bytes());
        for (f, t) in [(1u32, 11u32), (2, 25)] {
            b.extend_from_slice(&f.to_le_bytes());
            b.extend_from_slice(&t.to_le_bytes());
        }
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [2101u32, 10, 5, 30] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0);
        let h = parse_cbcl_header(&b, "x").unwrap();
        assert_eq!(h.bins, vec![0, 11, 25, 0]);
        assert_eq!(h.tiles[0].offset, 45);
        for n in 0..b.len() {
            let _ = parse_cbcl_header(&b[..n], "x");
        }
        assert!(parse_filter(&[0; 5], "f").is_err());
        assert_eq!(fastq_coordinate(826.1641), 9262);
        assert_eq!(surface_of_tile(2101), 2);
        assert_eq!(surface_of_tile(11101), 1);
    }

    #[test]
    fn nibbles_from_an_offset() {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&[0x21, 0x43, 0x65]).unwrap();
        let z = e.finish().unwrap();
        assert_eq!(decode_nibbles(&z, 0, 6, "t").unwrap(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(decode_nibbles(&z, 1, 3, "t").unwrap(), vec![2, 3, 4]);
        assert!(decode_nibbles(&z, 4, 5, "t").is_err());
    }
}
