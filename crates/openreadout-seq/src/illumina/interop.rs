//! InterOp binary metrics (`InterOp/*MetricsOut.bin`): tile, quality, error, extraction,
//! corrected-intensity, extended-tile and index metrics, aggregated while they are read.
//!
//! Every file starts with a version byte and (except index metrics) a record-size byte; some
//! versions add an extended header; then fixed-size records follow, each starting with lane, tile
//! and (for per-cycle metrics) cycle. Layouts: `docs/formats/illumina-run.md`. Records are not
//! kept: they are summed into per-lane-and-cycle and per-tile totals as they are read, so memory
//! stays small on runs with millions of records. A record repeated for the same lane, tile and
//! cycle counts once.

use std::collections::{BTreeMap, HashSet};

use super::FORMAT_ID;

/// Largest InterOp file read (a NovaSeq S4 Q-metrics file is a few hundred MB at most).
pub const MAX_INTEROP_FILE: u64 = 2 << 30;

/// A quality-score bin: scores `low..=high` are reported as `value`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QBin {
    /// Lowest score of the bin.
    pub low: u8,
    /// Highest score of the bin.
    pub high: u8,
    /// The score reported for the bin.
    pub value: u8,
}

/// Totals for one lane and cycle, summed over tiles.
#[derive(Debug, Clone, Default)]
pub struct CycleTotals {
    /// Base calls per quality score (index = score, 0..=50).
    pub q_hist: Vec<u64>,
    /// Tiles with quality records.
    pub q_tiles: u32,
    /// Sum of per-tile error rates (percent) and the number of tiles.
    pub error_sum: f64,
    /// Tiles with an error rate.
    pub error_tiles: u32,
    /// Sum of per-tile 90th-percentile intensities per channel.
    pub intensity_sum: Vec<f64>,
    /// Tiles with extraction records.
    pub intensity_tiles: u32,
    /// Sum of per-tile focus (FWHM) per channel.
    pub focus_sum: Vec<f64>,
    /// Base calls per base: no call, A, C, G, T (corrected-intensity metrics).
    pub calls: [u64; 5],
}

/// Per-tile totals (tile metrics plus quality and error totals over cycles).
#[derive(Debug, Clone, Default)]
pub struct TileTotals {
    /// Cluster density, clusters per mm².
    pub density: Option<f64>,
    /// Density of clusters passing filter, clusters per mm².
    pub density_pf: Option<f64>,
    /// Clusters.
    pub clusters: Option<f64>,
    /// Clusters passing filter.
    pub clusters_pf: Option<f64>,
    /// Occupied wells (patterned flow cells; extended tile metrics).
    pub occupied: Option<f64>,
    /// Phasing per read (percent), keyed by read number.
    pub phasing: BTreeMap<u32, f64>,
    /// Prephasing per read (percent).
    pub prephasing: BTreeMap<u32, f64>,
    /// Percent of clusters aligned to PhiX per read.
    pub aligned: BTreeMap<u32, f64>,
    /// Base calls at Q30 or above and in total, per read (from the quality metrics).
    pub q30: BTreeMap<u32, (u64, u64)>,
    /// Sum of error rates (percent) and the number of cycles, per read.
    pub error: BTreeMap<u32, (f64, u32)>,
}

/// Clusters assigned to one index sequence (index metrics).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexCount {
    /// Lane.
    pub lane: u16,
    /// Index sequence (`I7+I5`).
    pub index: String,
    /// Sample name.
    pub sample: String,
    /// Project name.
    pub project: String,
    /// Clusters, summed over tiles.
    pub clusters: u64,
}

/// Every InterOp file of a run, aggregated.
#[derive(Debug, Clone, Default)]
pub struct InterOp {
    /// `(lane, cycle)` → totals.
    pub cycles: BTreeMap<(u16, u32), CycleTotals>,
    /// `(lane, tile)` → totals.
    pub tiles: BTreeMap<(u16, u32), TileTotals>,
    /// Quality bins, when the instrument bins scores.
    pub q_bins: Vec<QBin>,
    /// Tile area in mm² (tile metrics v3).
    pub tile_area_mm2: Option<f64>,
    /// Intensity channel count (4 for A, C, G, T on four-colour instruments).
    pub channels: usize,
    /// Clusters per index sequence and sample, per lane.
    pub index: Vec<IndexCount>,
    /// File name → format version, for every file read.
    pub versions: BTreeMap<String, u8>,
    /// Files present but not decoded (name → why).
    pub skipped: BTreeMap<String, String>,
    /// Problems met (truncated files, unknown versions): one line each.
    pub problems: Vec<String>,
}

/// Which read a cycle belongs to: `reads[i] = (read number, first cycle, last cycle)`.
pub type ReadLayout = [(u32, u32, u32)];

/// The read number of `cycle`, when it falls in one.
pub fn read_of(layout: &ReadLayout, cycle: u32) -> Option<u32> {
    layout
        .iter()
        .find(|(_, a, b)| (*a..=*b).contains(&cycle))
        .map(|(r, _, _)| *r)
}

/// Bounds-checked little-endian reader over one record or header.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, at: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn text(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        self.take(n).map(|s| String::from_utf8_lossy(s).into_owned())
    }
}

/// The records of a file with `header` bytes before them and records of `size` bytes; a
/// trailing partial record (a file still being written) is reported and left out.
fn records<'a>(
    name: &str,
    b: &'a [u8],
    header: usize,
    size: usize,
    problems: &mut Vec<String>,
) -> impl Iterator<Item = &'a [u8]> {
    let body = b.get(header..).unwrap_or(&[]);
    let size = size.max(1);
    let rem = body.len() % size;
    if rem != 0 {
        problems.push(format!(
            "{name}: ends with a partial record ({rem} of {size} bytes left out)"
        ));
    }
    body.chunks_exact(size)
}

impl InterOp {
    fn seen_once(seen: &mut HashSet<(u16, u32, u32)>, key: (u16, u32, u32)) -> bool {
        seen.insert(key)
    }

    fn cycle(&mut self, lane: u16, cycle: u32) -> &mut CycleTotals {
        self.cycles.entry((lane, cycle)).or_default()
    }

    fn tile(&mut self, lane: u16, tile: u32) -> &mut TileTotals {
        self.tiles.entry((lane, tile)).or_default()
    }

    /// Tile metrics (`TileMetricsOut.bin`), versions 2 and 3.
    pub fn add_tile(&mut self, name: &str, b: &[u8], layout: &ReadLayout) {
        let Some((&ver, rest)) = b.split_first() else {
            return;
        };
        self.versions.insert(name.into(), ver);
        let size = rest.first().copied().unwrap_or(0) as usize;
        match ver {
            2 if size == 10 => {
                let recs: Vec<&[u8]> = records(name, b, 2, 10, &mut self.problems).collect();
                for r in recs {
                    let mut c = Rd::new(r);
                    let (Some(lane), Some(tile), Some(code), Some(v)) =
                        (c.u16(), c.u16(), c.u16(), c.f32())
                    else {
                        continue;
                    };
                    let v = f64::from(v);
                    let t = self.tile(lane, u32::from(tile));
                    match code {
                        100 => t.density = Some(v),
                        101 => t.density_pf = Some(v),
                        102 => t.clusters = Some(v),
                        103 => t.clusters_pf = Some(v),
                        200..=299 => {
                            let read = u32::from(code - 200) / 2 + 1;
                            if layout.iter().any(|(n, ..)| *n == read) || layout.is_empty() {
                                if code % 2 == 0 {
                                    t.phasing.insert(read, v * 100.0);
                                } else {
                                    t.prephasing.insert(read, v * 100.0);
                                }
                            }
                        }
                        300..=399 => {
                            t.aligned.insert(u32::from(code - 300) + 1, v);
                        }
                        _ => {}
                    }
                }
            }
            3 if size == 15 => {
                let area = Rd::new(b.get(2..).unwrap_or(&[])).f32();
                if let Some(a) = area.filter(|a| a.is_finite() && *a > 0.0) {
                    self.tile_area_mm2 = Some(f64::from(a));
                }
                let recs: Vec<&[u8]> = records(name, b, 6, 15, &mut self.problems).collect();
                let area = self.tile_area_mm2;
                for r in recs {
                    let mut c = Rd::new(r);
                    let (Some(lane), Some(tile), Some(code)) = (c.u16(), c.u32(), c.u8()) else {
                        continue;
                    };
                    match code {
                        b't' => {
                            let (Some(n), Some(pf)) = (c.f32(), c.f32()) else {
                                continue;
                            };
                            let t = self.tile(lane, tile);
                            t.clusters = Some(f64::from(n));
                            t.clusters_pf = Some(f64::from(pf));
                            if let Some(a) = area {
                                t.density = Some(f64::from(n) / a);
                                t.density_pf = Some(f64::from(pf) / a);
                            }
                        }
                        b'r' => {
                            let (Some(read), Some(al)) = (c.u32(), c.f32()) else {
                                continue;
                            };
                            if al.is_finite() {
                                self.tile(lane, tile).aligned.insert(read, f64::from(al));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                self.skipped.insert(
                    name.into(),
                    format!("tile metrics version {ver} (record size {size}) is not decoded"),
                );
            }
        }
    }

    /// Extended tile metrics (`ExtendedTileMetricsOut.bin`): occupied wells.
    pub fn add_extended_tile(&mut self, name: &str, b: &[u8]) {
        let (Some(&ver), Some(&size)) = (b.first(), b.get(1)) else {
            return;
        };
        self.versions.insert(name.into(), ver);
        if size < 10 || ver > 3 {
            self.skipped.insert(
                name.into(),
                format!("extended tile metrics version {ver} (record size {size}) is not decoded"),
            );
            return;
        }
        let recs: Vec<&[u8]> =
            records(name, b, 2, size as usize, &mut self.problems).collect();
        for r in recs {
            let mut c = Rd::new(r);
            let lane = c.u16();
            if ver == 1 {
                let (Some(lane), Some(tile), Some(code), Some(v)) = (lane, c.u16(), c.u16(), c.f32())
                else {
                    continue;
                };
                if code == 0 {
                    self.tile(lane, u32::from(tile)).occupied = Some(f64::from(v));
                }
            } else {
                let (Some(lane), Some(tile), Some(v)) = (lane, c.u32(), c.f32()) else {
                    continue;
                };
                if v.is_finite() {
                    self.tile(lane, tile).occupied = Some(f64::from(v));
                }
            }
        }
    }

    /// Quality metrics (`QMetricsOut.bin`), versions 4 to 7.
    pub fn add_q(&mut self, name: &str, b: &[u8], layout: &ReadLayout) {
        let (Some(&ver), Some(&size)) = (b.first(), b.get(1)) else {
            return;
        };
        self.versions.insert(name.into(), ver);
        let mut header = 2usize;
        let mut bins: Vec<QBin> = Vec::new();
        if ver >= 5 {
            let has_bins = b.get(2).copied().unwrap_or(0) != 0;
            header = 3;
            if has_bins {
                let n = b.get(3).copied().unwrap_or(0) as usize;
                header = 4 + 3 * n;
                let h = b.get(4..header).unwrap_or(&[]);
                if h.len() == 3 * n {
                    for i in 0..n {
                        let bin = if ver >= 7 {
                            QBin {
                                low: h[3 * i],
                                high: h[3 * i + 1],
                                value: h[3 * i + 2],
                            }
                        } else {
                            QBin {
                                low: h[i],
                                high: h[n + i],
                                value: h[2 * n + i],
                            }
                        };
                        bins.push(bin);
                    }
                }
            }
        }
        let tile32 = ver >= 7;
        let base = if tile32 { 8 } else { 6 };
        let size = size as usize;
        if !(4..=7).contains(&ver) || size < base || (size - base) % 4 != 0 {
            self.skipped.insert(
                name.into(),
                format!("quality metrics version {ver} (record size {size}) is not decoded"),
            );
            return;
        }
        let entries = (size - base) / 4;
        // The score each histogram entry stands for: entry i is Q(i+1) for 50 unbinned
        // entries (v4, v5), else the bin's value.
        let score_of: Vec<u8> = if ver >= 6 && !bins.is_empty() && entries == bins.len() {
            bins.iter().map(|q| q.value).collect()
        } else {
            (1..=entries).map(|i| i.min(50) as u8).collect()
        };
        if !bins.is_empty() {
            self.q_bins = bins;
        }
        let mut seen = HashSet::new();
        let recs: Vec<&[u8]> = records(name, b, header, size, &mut self.problems).collect();
        for r in recs {
            let mut c = Rd::new(r);
            let lane = c.u16();
            let tile = if tile32 { c.u32() } else { c.u16().map(u32::from) };
            let cycle = c.u16();
            let (Some(lane), Some(tile), Some(cycle)) = (lane, tile, cycle) else {
                continue;
            };
            let cycle = u32::from(cycle);
            if !Self::seen_once(&mut seen, (lane, tile, cycle)) {
                continue;
            }
            let mut q30 = 0u64;
            let mut total = 0u64;
            let ct = self.cycle(lane, cycle);
            if ct.q_hist.len() < 51 {
                ct.q_hist.resize(51, 0);
            }
            ct.q_tiles += 1;
            for &s in &score_of {
                let Some(n) = c.u32() else { break };
                let n = u64::from(n);
                ct.q_hist[s.min(50) as usize] += n;
                total += n;
                if s >= 30 {
                    q30 += n;
                }
            }
            if let Some(read) = read_of(layout, cycle) {
                let e = self.tile(lane, tile).q30.entry(read).or_default();
                e.0 += q30;
                e.1 += total;
            }
        }
    }

    /// Error metrics (`ErrorMetricsOut.bin`), versions 3 to 6.
    pub fn add_error(&mut self, name: &str, b: &[u8], layout: &ReadLayout) {
        let (Some(&ver), Some(&size)) = (b.first(), b.get(1)) else {
            return;
        };
        self.versions.insert(name.into(), ver);
        let (header, tile32) = match ver {
            3 => (2, false),
            4 | 5 => (2, true),
            6 => {
                let n = Rd::new(b.get(2..).unwrap_or(&[])).u16().unwrap_or(0) as usize;
                let len = Rd::new(b.get(4..).unwrap_or(&[])).u16().unwrap_or(0) as usize;
                (6 + n.saturating_mul(len), true)
            }
            _ => {
                self.skipped.insert(
                    name.into(),
                    format!("error metrics version {ver} is not decoded"),
                );
                return;
            }
        };
        let mut seen = HashSet::new();
        let recs: Vec<&[u8]> =
            records(name, b, header, size as usize, &mut self.problems).collect();
        for r in recs {
            let mut c = Rd::new(r);
            let lane = c.u16();
            let tile = if tile32 { c.u32() } else { c.u16().map(u32::from) };
            let (Some(lane), Some(tile), Some(cycle), Some(rate)) = (lane, tile, c.u16(), c.f32())
            else {
                continue;
            };
            let cycle = u32::from(cycle);
            if !rate.is_finite() || !Self::seen_once(&mut seen, (lane, tile, cycle)) {
                continue;
            }
            let rate = f64::from(rate);
            let ct = self.cycle(lane, cycle);
            ct.error_sum += rate;
            ct.error_tiles += 1;
            if let Some(read) = read_of(layout, cycle) {
                let e = self.tile(lane, tile).error.entry(read).or_default();
                e.0 += rate;
                e.1 += 1;
            }
        }
    }

    /// Extraction metrics (`ExtractionMetricsOut.bin`), versions 2 and 3: focus and the 90th
    /// percentile intensity per channel.
    pub fn add_extraction(&mut self, name: &str, b: &[u8]) {
        let (Some(&ver), Some(&size)) = (b.first(), b.get(1)) else {
            return;
        };
        self.versions.insert(name.into(), ver);
        let (header, channels, tile32) = match ver {
            2 if size == 38 => (2, 4usize, false),
            3 => {
                let n = b.get(2).copied().unwrap_or(0) as usize;
                if size as usize != 8 + 6 * n || n == 0 {
                    self.skipped.insert(
                        name.into(),
                        format!("extraction metrics v3 with record size {size} and {n} channels"),
                    );
                    return;
                }
                (3, n, true)
            }
            _ => {
                self.skipped.insert(
                    name.into(),
                    format!("extraction metrics version {ver} (record size {size}) is not decoded"),
                );
                return;
            }
        };
        self.channels = self.channels.max(channels);
        let mut seen = HashSet::new();
        let recs: Vec<&[u8]> =
            records(name, b, header, size as usize, &mut self.problems).collect();
        for r in recs {
            let mut c = Rd::new(r);
            let lane = c.u16();
            let tile = if tile32 { c.u32() } else { c.u16().map(u32::from) };
            let (Some(lane), Some(tile), Some(cycle)) = (lane, tile, c.u16()) else {
                continue;
            };
            let cycle = u32::from(cycle);
            if !Self::seen_once(&mut seen, (lane, tile, cycle)) {
                continue;
            }
            let focus: Vec<f64> = (0..channels)
                .filter_map(|_| c.f32().map(f64::from))
                .collect();
            let max: Vec<f64> = (0..channels)
                .filter_map(|_| c.u16().map(f64::from))
                .collect();
            if focus.len() != channels || max.len() != channels {
                continue;
            }
            let ct = self.cycle(lane, cycle);
            ct.intensity_sum.resize(channels, 0.0);
            ct.focus_sum.resize(channels, 0.0);
            for i in 0..channels {
                ct.intensity_sum[i] += max[i];
                if focus[i].is_finite() {
                    ct.focus_sum[i] += focus[i];
                }
            }
            ct.intensity_tiles += 1;
        }
    }

    /// Corrected-intensity metrics (`CorrectedIntMetricsOut.bin`), versions 2 to 4: base calls
    /// per base.
    pub fn add_corrected(&mut self, name: &str, b: &[u8]) {
        let (Some(&ver), Some(&size)) = (b.first(), b.get(1)) else {
            return;
        };
        self.versions.insert(name.into(), ver);
        let (skip, tile32) = match (ver, size) {
            (2, 48) => (18usize, false),
            (3, 34) => (8, false),
            (4, 28) => (0, true),
            _ => {
                self.skipped.insert(
                    name.into(),
                    format!("corrected intensity metrics version {ver} (record size {size}) is not decoded"),
                );
                return;
            }
        };
        let mut seen = HashSet::new();
        let recs: Vec<&[u8]> = records(name, b, 2, size as usize, &mut self.problems).collect();
        for r in recs {
            let mut c = Rd::new(r);
            let lane = c.u16();
            let tile = if tile32 { c.u32() } else { c.u16().map(u32::from) };
            let (Some(lane), Some(tile), Some(cycle)) = (lane, tile, c.u16()) else {
                continue;
            };
            let cycle = u32::from(cycle);
            if c.take(skip).is_none() || !Self::seen_once(&mut seen, (lane, tile, cycle)) {
                continue;
            }
            let mut calls = [0u64; 5];
            for v in &mut calls {
                *v = u64::from(c.u32().unwrap_or(0));
            }
            let ct = self.cycle(lane, cycle);
            for (a, v) in ct.calls.iter_mut().zip(calls) {
                *a += v;
            }
        }
    }

    /// Index metrics (`IndexMetricsOut.bin`), versions 1 and 2.
    pub fn add_index(&mut self, name: &str, b: &[u8]) {
        let Some(&ver) = b.first() else {
            return;
        };
        self.versions.insert(name.into(), ver);
        if !(1..=2).contains(&ver) {
            self.skipped.insert(
                name.into(),
                format!("index metrics version {ver} is not decoded"),
            );
            return;
        }
        let mut c = Rd::new(&b[1..]);
        let mut sums: BTreeMap<(u16, String, String, String), u64> = BTreeMap::new();
        loop {
            if c.at >= c.b.len() {
                break;
            }
            let lane = c.u16();
            let tile = if ver == 2 { c.u32() } else { c.u16().map(u32::from) };
            let read = c.u16();
            let index = c.text();
            let count = if ver == 2 {
                c.u64()
            } else {
                c.u32().map(u64::from)
            };
            let sample = c.text();
            let project = c.text();
            let (Some(lane), Some(_), Some(_), Some(index), Some(count), Some(sample), Some(project)) =
                (lane, tile, read, index, count, sample, project)
            else {
                self.problems
                    .push(format!("{name}: ends inside a record (the rest is left out)"));
                break;
            };
            *sums.entry((lane, index, sample, project)).or_default() += count;
        }
        self.index = sums
            .into_iter()
            .map(|((lane, index, sample, project), clusters)| IndexCount {
                lane,
                index,
                sample,
                project,
                clusters,
            })
            .collect();
    }
}

/// Mean and population standard deviation of finite values.
pub fn mean_sd(v: impl IntoIterator<Item = f64>) -> Option<(f64, f64)> {
    let v: Vec<f64> = v.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    // sample standard deviation, as the instrument's run summary reports it
    let var = if v.len() > 1 {
        v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    Some((m, var.sqrt()))
}

/// The format id, for errors raised while reading metrics.
pub const METRICS_FORMAT: &str = FORMAT_ID;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_v7_bins_and_records() {
        // version 7, record size 8 + 4*3, has bins, 3 bins as triples
        let mut b = vec![7u8, 20, 1, 3, 2, 17, 11, 18, 29, 25, 30, 40, 37];
        for (lane, tile, cycle, h) in [(1u16, 2101u32, 1u16, [10u32, 20, 70])] {
            b.extend_from_slice(&lane.to_le_bytes());
            b.extend_from_slice(&tile.to_le_bytes());
            b.extend_from_slice(&cycle.to_le_bytes());
            for v in h {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut io = InterOp::default();
        io.add_q("QMetricsOut.bin", &b, &[(1, 1, 36)]);
        assert_eq!(io.q_bins.len(), 3);
        assert_eq!(io.q_bins[2].value, 37);
        let c = &io.cycles[&(1, 1)];
        assert_eq!(c.q_hist[37], 70);
        assert_eq!(io.tiles[&(1, 2101)].q30[&1], (70, 100));
        assert!(io.problems.is_empty());
    }

    #[test]
    fn truncated_and_garbage_input_never_panics() {
        let mut io = InterOp::default();
        for len in 0..64 {
            let b: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            let mut b2 = b.clone();
            if let Some(x) = b2.first_mut() {
                *x = 7;
            }
            io.add_q("q", &b2, &[]);
            io.add_tile("t", &b, &[]);
            io.add_error("e", &b, &[]);
            io.add_extraction("x", &b);
            io.add_corrected("c", &b);
            io.add_index("i", &b);
            io.add_extended_tile("et", &b);
        }
    }
}
