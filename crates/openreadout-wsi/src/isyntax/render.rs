//! Decoding tiles of an iSyntax pyramid. A tile of scale s needs the high-pass coefficients of
//! its 3 × 3 neighbourhood and their LL coefficients, which come from the inverse transform of
//! their parent tiles at scale s + 1 (or from the LL code blocks at the top scale). Work is
//! planned scale by scale from the top: `docs/formats/philips-isyntax.md` § Reconstruction.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use openreadout_core::region::TileCache;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use rayon::prelude::*;

use super::FORMAT_ID;
use super::codec::decode_block;
use super::pyramid::Pyramid;
use super::wavelet::{Neighbour, child_ll, tile_component, to_rgb};

/// A tile key: scale, column, row.
pub type TileKey = (u32, i64, i64);
/// Coefficients of the three colour components of one tile, one after the other.
pub type Coeffs = Vec<i16>;

/// Byte budget of each coefficient cache and of the RGB tile cache.
const CACHE_BYTES: usize = 384 << 20;

/// Decodes tiles, keeping coefficients and RGB tiles between calls.
pub struct Decoder {
    pyramid: Arc<Pyramid>,
    file: SourceFile,
    /// (offset, size) of each code block, from the seek table.
    blocks: Arc<Vec<(u64, u64)>>,
    ll: TileCache<TileKey, Coeffs>,
    high: TileCache<TileKey, Coeffs>,
    rgb: TileCache<TileKey, Vec<u8>>,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

fn neighbours(k: TileKey) -> impl Iterator<Item = TileKey> {
    (-1..=1).flat_map(move |dy| (-1..=1).map(move |dx| (k.0, k.1 + dx, k.2 + dy)))
}

impl Decoder {
    /// A decoder reading code blocks from `file`.
    pub fn new(pyramid: Arc<Pyramid>, file: SourceFile, blocks: Vec<(u64, u64)>) -> Decoder {
        Decoder {
            pyramid,
            file,
            blocks: Arc::new(blocks),
            ll: TileCache::new(CACHE_BYTES),
            high: TileCache::new(CACHE_BYTES),
            rgb: TileCache::new(CACHE_BYTES),
        }
    }

    /// Decode the three colour blocks of a tile (LL or high-pass).
    fn decode_tile_blocks(
        p: &Pyramid,
        file: &SourceFile,
        blocks: &[(u64, u64)],
        k: TileKey,
        ll: bool,
    ) -> Result<Coeffs> {
        let (bw, bh) = (p.block_w as usize, p.block_h as usize);
        let per = if ll { 1 } else { 3 } * bw * bh;
        let mut out = Vec::with_capacity(3 * per);
        for color in 0..3u8 {
            let idx = p
                .block(k.0, ll, color, k.1 as u32, k.2 as u32)
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "tile ({}, {}) of scale {} has no {} code block for colour {color}",
                            k.1,
                            k.2,
                            k.0,
                            if ll { "LL" } else { "high-pass" }
                        ),
                    )
                })?;
            let (offset, size) = blocks[idx as usize];
            let mut data = vec![0u8; size as usize];
            if size > 0 {
                file.read_exact_at(offset, &mut data).map_err(|e| {
                    Error::corrupt_at(FORMAT_ID, offset, format!("code block {idx}: {e}"))
                })?;
            }
            let c = decode_block(&data, bw, bh, ll).map_err(|e| {
                Error::corrupt_at(FORMAT_ID, offset, format!("code block {idx}: {e}"))
            })?;
            out.extend_from_slice(&c);
        }
        Ok(out)
    }

    /// RGB pixels (`2·bw × 2·bh`, interleaved) of every tile in `want` (all at scale `s`, all
    /// existing).
    pub fn tiles(&mut self, s: u32, want: &[TileKey]) -> Result<HashMap<TileKey, Arc<Vec<u8>>>> {
        let mut out = HashMap::new();
        let mut todo = Vec::new();
        for &k in want {
            match self.rgb.get(&k) {
                Some(t) => {
                    out.insert(k, t);
                }
                None => todo.push(k),
            }
        }
        if todo.is_empty() {
            return Ok(out);
        }
        let p = self.pyramid.clone();
        let top = p.levels - 1;
        // Plan: tiles to transform per scale, from `s` up. LL blocks already cached are pinned
        // in `ll_local` so that no eviction during this call loses them.
        let mut ll_local: HashMap<TileKey, Arc<Coeffs>> = HashMap::new();
        let mut transform: Vec<HashSet<TileKey>> = vec![HashSet::new(); p.levels as usize];
        transform[s as usize] = todo.iter().copied().collect();
        let mut need: Vec<HashSet<TileKey>> = vec![HashSet::new(); p.levels as usize];
        for sc in s..=top {
            let nb: HashSet<TileKey> = transform[sc as usize]
                .iter()
                .flat_map(|&k| neighbours(k))
                .filter(|n| p.exists(n.0, n.1, n.2))
                .collect();
            for &n in &nb {
                if let Some(c) = self.ll.get(&n) {
                    ll_local.insert(n, c);
                } else if sc < top {
                    let q = (sc + 1, n.1.div_euclid(2), n.2.div_euclid(2));
                    if p.exists(q.0, q.1, q.2) {
                        transform[sc as usize + 1].insert(q);
                    }
                }
            }
            need[sc as usize] = nb;
        }
        // Run from the top scale down.
        for sc in (s..=top).rev() {
            let nb = &need[sc as usize];
            let (file, blocks) = (&self.file, &self.blocks);
            // High-pass coefficients of the neighbourhood.
            let mut high: HashMap<TileKey, Arc<Coeffs>> = HashMap::new();
            let mut missing = Vec::new();
            for &k in nb {
                match self.high.get(&k) {
                    Some(c) => {
                        high.insert(k, c);
                    }
                    None => missing.push(k),
                }
            }
            let fresh: Vec<Result<(TileKey, Coeffs)>> = missing
                .par_iter()
                .map(|&k| Self::decode_tile_blocks(&p, file, blocks, k, false).map(|c| (k, c)))
                .collect();
            for r in fresh {
                let (k, c) = r?;
                let c = Arc::new(c);
                self.high.put(k, c.clone(), c.len() * 2);
                high.insert(k, c);
            }
            // LL coefficients: stored at the top scale, else from the parents' transforms (in
            // `ll_local` or the cache); `None` when the parent tile does not exist.
            let mut lls: HashMap<TileKey, Arc<Coeffs>> = HashMap::new();
            let mut missing = Vec::new();
            for &k in nb {
                if let Some(c) = ll_local.get(&k).cloned() {
                    lls.insert(k, c);
                } else if sc == top {
                    missing.push(k);
                }
            }
            let fresh: Vec<Result<(TileKey, Coeffs)>> = missing
                .par_iter()
                .map(|&k| Self::decode_tile_blocks(&p, file, blocks, k, true).map(|c| (k, c)))
                .collect();
            for r in fresh {
                let (k, c) = r?;
                let c = Arc::new(c);
                self.ll.put(k, c.clone(), c.len() * 2);
                lls.insert(k, c);
            }
            // Transform this scale's tiles.
            let list: Vec<TileKey> = transform[sc as usize].iter().copied().collect();
            let (bw, bh) = (p.block_w as usize, p.block_h as usize);
            let results: Vec<(TileKey, [Vec<i16>; 3])> = list
                .par_iter()
                .map(|&k| {
                    let comps: [Vec<i16>; 3] = std::array::from_fn(|color| {
                        let area = bw * bh;
                        let cells: [Option<Neighbour<'_>>; 9] = std::array::from_fn(|i| {
                            let n = (k.0, k.1 + (i % 3) as i64 - 1, k.2 + (i / 3) as i64 - 1);
                            let h = high.get(&n)?;
                            Some(Neighbour {
                                ll: lls.get(&n).map(|l| &l[color * area..(color + 1) * area]),
                                h: &h[color * 3 * area..(color + 1) * 3 * area],
                            })
                        });
                        tile_component(&cells, bw, bh, color == 0)
                    });
                    (k, comps)
                })
                .collect();
            for (k, comps) in results {
                if sc == s {
                    let px = Arc::new(to_rgb(&comps[0], &comps[1], &comps[2], bw, bh));
                    self.rgb.put(k, px.clone(), px.len());
                    out.insert(k, px);
                }
                if sc > s {
                    let kids: Vec<[Vec<i16>; 4]> =
                        comps.iter().map(|c| child_ll(c, bw, bh)).collect();
                    for (q, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                        let ck = (sc - 1, 2 * k.1 + dx, 2 * k.2 + dy);
                        let mut c = Vec::with_capacity(3 * bw * bh);
                        for comp in &kids {
                            c.extend_from_slice(&comp[q]);
                        }
                        let c = Arc::new(c);
                        self.ll.put(ck, c.clone(), c.len() * 2);
                        ll_local.insert(ck, c);
                    }
                }
            }
            ll_local.retain(|k, _| k.0 < sc);
        }
        Ok(out)
    }
}
