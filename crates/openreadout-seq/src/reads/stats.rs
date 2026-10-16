//! Read statistics accumulated while streaming FASTQ or BAM records: counts, bases, lengths
//! (N50), qualities (per read, per position, Q20/Q30 bases) and base composition.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::table::{Col, MemTable, tidy};

/// Positions tracked exactly for the per-position quality profile.
pub const MAX_POSITIONS: usize = 100_000;
/// Rows of the per-position quality table (positions are binned beyond this).
pub const POSITION_ROWS: usize = 1000;

/// Phred quality to error probability, precomputed.
fn error_prob(q: u8) -> f64 {
    10f64.powf(-f64::from(q) / 10.0)
}

/// Accumulated statistics.
#[derive(Debug, Clone)]
pub struct ReadStats {
    /// Records seen.
    pub reads: u64,
    /// Bases seen.
    pub bases: u64,
    /// Read length → reads.
    pub lengths: BTreeMap<u64, u64>,
    /// Bases at Q20 or above.
    pub q20: u64,
    /// Bases at Q30 or above.
    pub q30: u64,
    /// Bases with a quality.
    pub qual_bases: u64,
    /// Reads without qualities (BAM `*`).
    pub no_quality: u64,
    /// Mean read quality (rounded down) → reads.
    pub read_quality: BTreeMap<u32, u64>,
    /// A, C, G, T, N and other bases.
    pub composition: [u64; 6],
    /// Sum of quality and bases per position (first `MAX_POSITIONS`).
    pub pos_q: Vec<(u64, u64)>,
    /// Smallest quality character seen (FASTQ), for the encoding.
    pub min_qchar: u8,
    /// Largest quality character seen.
    pub max_qchar: u8,
    table: [f64; 94],
}

impl Default for ReadStats {
    fn default() -> Self {
        let mut table = [0.0; 94];
        for (q, t) in table.iter_mut().enumerate() {
            *t = error_prob(q as u8);
        }
        ReadStats {
            reads: 0,
            bases: 0,
            lengths: BTreeMap::new(),
            q20: 0,
            q30: 0,
            qual_bases: 0,
            no_quality: 0,
            read_quality: BTreeMap::new(),
            composition: [0; 6],
            pos_q: Vec::new(),
            min_qchar: u8::MAX,
            max_qchar: 0,
            table,
        }
    }
}

/// Per-read values the reads table keeps.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadValues {
    /// Length in bases.
    pub length: u64,
    /// Mean quality: the Phred value of the mean error probability (NaN without qualities).
    pub mean_quality: f64,
    /// GC content in percent of A, C, G and T bases.
    pub gc_percent: f64,
    /// N bases.
    pub n_count: u64,
}

/// The base-composition slot of a base letter.
fn slot(b: u8) -> usize {
    match b {
        b'A' | b'a' => 0,
        b'C' | b'c' => 1,
        b'G' | b'g' => 2,
        b'T' | b't' => 3,
        b'N' | b'n' => 4,
        _ => 5,
    }
}

impl ReadStats {
    /// Add one read: its bases as letters and its qualities as Phred values (empty when it has
    /// none).
    pub fn add(&mut self, seq: &[u8], qual: &[u8]) -> ReadValues {
        let len = seq.len() as u64;
        self.reads += 1;
        self.bases += len;
        *self.lengths.entry(len).or_default() += 1;
        let mut comp = [0u64; 6];
        for &b in seq {
            comp[slot(b)] += 1;
        }
        for (a, c) in self.composition.iter_mut().zip(comp) {
            *a += c;
        }
        let acgt = comp[0] + comp[1] + comp[2] + comp[3];
        let gc = if acgt > 0 {
            (comp[1] + comp[2]) as f64 / acgt as f64 * 100.0
        } else {
            f64::NAN
        };
        let mean_q = if qual.is_empty() {
            self.no_quality += 1;
            f64::NAN
        } else {
            let mut err = 0.0;
            if self.pos_q.len() < qual.len().min(MAX_POSITIONS) {
                self.pos_q.resize(qual.len().min(MAX_POSITIONS), (0, 0));
            }
            for (i, &q) in qual.iter().enumerate() {
                let q = q.min(93);
                err += self.table[q as usize];
                if q >= 20 {
                    self.q20 += 1;
                }
                if q >= 30 {
                    self.q30 += 1;
                }
                if let Some(p) = self.pos_q.get_mut(i) {
                    p.0 += u64::from(q);
                    p.1 += 1;
                }
            }
            self.qual_bases += qual.len() as u64;
            let mean_err = err / qual.len() as f64;
            let q = -10.0 * mean_err.max(1e-10).log10();
            *self
                .read_quality
                .entry(q.floor().max(0.0) as u32)
                .or_default() += 1;
            q
        };
        ReadValues {
            length: len,
            mean_quality: mean_q,
            gc_percent: gc,
            n_count: comp[4],
        }
    }

    /// The length at which reads of that length or longer hold half the bases.
    pub fn n50(&self) -> Option<u64> {
        let half = self.bases.div_ceil(2);
        let mut acc = 0u64;
        for (len, n) in self.lengths.iter().rev() {
            acc += len * n;
            if acc >= half {
                return Some(*len);
            }
        }
        None
    }

    fn quantile_len(&self, f: f64) -> Option<u64> {
        if self.reads == 0 {
            return None;
        }
        let target = ((self.reads as f64 - 1.0) * f).round() as u64;
        let mut acc = 0u64;
        for (len, n) in &self.lengths {
            acc += n;
            if acc > target {
                return Some(*len);
            }
        }
        self.lengths.keys().next_back().copied()
    }

    /// The summary for `info`.
    pub fn summary(&self) -> Value {
        let total_comp: u64 = self.composition.iter().sum();
        let acgt: u64 = self.composition[..4].iter().sum();
        json!({
            "reads": self.reads,
            "bases": self.bases,
            "length_min": self.lengths.keys().next(),
            "length_max": self.lengths.keys().next_back(),
            "length_mean": (self.reads > 0).then(|| tidy(self.bases as f64 / self.reads as f64, 6)),
            "length_median": self.quantile_len(0.5),
            "length_n50": self.n50(),
            "percent_q20": (self.qual_bases > 0).then(|| tidy(self.q20 as f64 / self.qual_bases as f64 * 100.0, 5)),
            "percent_q30": (self.qual_bases > 0).then(|| tidy(self.q30 as f64 / self.qual_bases as f64 * 100.0, 5)),
            "mean_read_quality_median": self.median_read_quality(),
            "percent_gc": (acgt > 0).then(|| tidy((self.composition[1] + self.composition[2]) as f64 / acgt as f64 * 100.0, 5)),
            "percent_n": (total_comp > 0).then(|| tidy(self.composition[4] as f64 / total_comp as f64 * 100.0, 4)),
            "reads_without_quality": self.no_quality,
        })
    }

    fn median_read_quality(&self) -> Option<u32> {
        let n: u64 = self.read_quality.values().sum();
        if n == 0 {
            return None;
        }
        let mut acc = 0;
        for (q, c) in &self.read_quality {
            acc += c;
            if acc * 2 >= n {
                return Some(*q);
            }
        }
        None
    }

    /// Table of mean quality per position (binned to at most `POSITION_ROWS` rows).
    pub fn position_table(&self) -> MemTable {
        let mut t = MemTable::new(
            "quality by position",
            vec![
                Col::int("position"),
                Col::int("position_end"),
                Col::num("mean_quality", None),
                Col::num("bases", None),
            ],
        );
        let n = self.pos_q.len();
        let bin = n.div_ceil(POSITION_ROWS).max(1);
        let mut i = 0;
        while i < n {
            let end = (i + bin).min(n);
            let (s, c) = self.pos_q[i..end]
                .iter()
                .fold((0u64, 0u64), |a, b| (a.0 + b.0, a.1 + b.1));
            t.col("position").push(Some((i + 1) as f64));
            t.col("position_end").push(Some(end as f64));
            t.col("mean_quality")
                .push((c > 0).then(|| s as f64 / c as f64));
            t.col("bases").push(Some(c as f64));
            i = end;
        }
        t.extra.insert("kind".into(), json!("quality_by_position"));
        t.extra.insert("bin_width".into(), json!(bin));
        t
    }

    /// Table of read counts per length bin (about two bins per square root of the reads, 20 to
    /// 200, evenly spaced, or logarithmic when lengths span more than two decades).
    pub fn length_table(&self) -> MemTable {
        let mut t = MemTable::new(
            "read lengths",
            vec![
                Col::int("length_from"),
                Col::int("length_to"),
                Col::num("reads", None),
                Col::num("bases", None),
            ],
        );
        let (Some(&lo), Some(&hi)) = (self.lengths.keys().next(), self.lengths.keys().next_back())
        else {
            return t;
        };
        let log = lo > 0 && hi as f64 / lo.max(1) as f64 > 100.0;
        // about two bins per square root of the reads, 20 to 200 of them
        let nbins = ((self.reads as f64).sqrt() * 2.0).clamp(20.0, 200.0) as u64;
        let edges: Vec<u64> = if hi - lo < nbins {
            (lo..=hi + 1).collect()
        } else if log {
            let (a, b) = ((lo.max(1) as f64).ln(), ((hi + 1) as f64).ln());
            let k = nbins.min(100) as i32;
            let mut e: Vec<u64> = (0..=k)
                .map(|i| (a + (b - a) * f64::from(i) / f64::from(k)).exp().round() as u64)
                .collect();
            e.dedup();
            if e.last().is_some_and(|x| *x <= hi) {
                e.push(hi + 1);
            }
            e
        } else {
            let w = (hi - lo + 1).div_ceil(nbins);
            let mut e: Vec<u64> = (0..=(hi - lo) / w).map(|i| lo + i * w).collect();
            e.push(e.last().copied().unwrap_or(lo) + w);
            e
        };
        for w in edges.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (mut r, mut s) = (0u64, 0u64);
            for (len, n) in self.lengths.range(a..b) {
                r += n;
                s += len * n;
            }
            t.col("length_from").push(Some(a as f64));
            t.col("length_to").push(Some((b - 1) as f64));
            t.col("reads").push(Some(r as f64));
            t.col("bases").push(Some(s as f64));
        }
        t.extra.insert("kind".into(), json!("length_histogram"));
        t.extra.insert(
            "bins".into(),
            json!(if log { "logarithmic" } else { "linear" }),
        );
        t
    }

    /// Table of reads per mean read quality (whole Phred units).
    pub fn quality_table(&self) -> MemTable {
        let mut t = MemTable::new(
            "read quality",
            vec![Col::int("mean_quality"), Col::num("reads", None)],
        );
        for (q, n) in &self.read_quality {
            t.col("mean_quality").push(Some(f64::from(*q)));
            t.col("reads").push(Some(*n as f64));
        }
        t.extra
            .insert("kind".into(), json!("read_quality_histogram"));
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_and_n50() {
        let mut s = ReadStats::default();
        let v = s.add(b"ACGTNN", &[30, 30, 30, 30, 2, 2]);
        assert_eq!(v.length, 6);
        assert!((v.gc_percent - 50.0).abs() < 1e-9);
        assert_eq!(v.n_count, 2);
        s.add(b"AAAAAAAAAA", &[40; 10]);
        s.add(b"CC", &[]);
        assert_eq!(s.reads, 3);
        assert_eq!(s.n50(), Some(10));
        assert_eq!(s.q30, 14);
        assert_eq!(s.no_quality, 1);
        let lt = s.length_table();
        assert_eq!(lt.rows(), 9);
        assert_eq!(s.position_table().rows(), 10);
    }
}
