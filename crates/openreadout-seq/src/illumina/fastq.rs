//! FASTQ from the base calls of a run folder: one gzip-compressed file per lane and read
//! (`<run>_L001_R1.fastq.gz`, index reads as `I1`, `I2`), records in tile and cluster order,
//! clusters that failed the filter left out unless asked for. Read names follow the form the
//! conversion tools write: `@<instrument>:<run number>:<flowcell>:<lane>:<tile>:<x>:<y>
//! <read>:<filtered Y/N>:0:<index>`, coordinates `round(10 × position + 1000)` from the
//! cluster locations.
//!
//! Each file is written under a temporary name, read back (decompressed, records counted and
//! hashed against what was written) and only then renamed into place. Tiles are decoded in
//! windows of clusters so memory stays bounded whatever the read length.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use openreadout_core::{Error, Result};
use xxhash_rust::xxh3::Xxh3;

use super::basecalls::fastq_coordinate;
use super::{FORMAT_ID, IlluminaRun};

/// Bytes of decoded base calls held at once (clusters × cycles).
const WINDOW_BYTES: u64 = 128 << 20;

/// Options of [`export_fastq`].
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct FastqOptions {
    /// Lanes to write (empty: every lane with base calls).
    pub lanes: Vec<u16>,
    /// Tiles to write (empty: every tile).
    pub tiles: Vec<u32>,
    /// Also write clusters that failed the filter (their names say `Y`).
    pub include_filtered: bool,
    /// Stop after this many clusters per lane (a sample of the run).
    pub max_reads: Option<u64>,
    /// Replace existing output files.
    pub overwrite: bool,
}

/// One FASTQ file written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastqFile {
    /// Path of the file.
    pub path: PathBuf,
    /// Lane.
    pub lane: u16,
    /// `R1`, `R2`, `I1`, ….
    pub read: String,
    /// Records written.
    pub records: u64,
    /// Bases written.
    pub bases: u64,
    /// xxh3 of the uncompressed records.
    pub xxh3: String,
}

/// What [`export_fastq`] wrote.
#[derive(Debug, Clone, Default)]
pub struct FastqReport {
    /// Files written, in lane and read order.
    pub files: Vec<FastqFile>,
    /// Clusters written per lane (each cluster is one record in every read's file).
    pub clusters: u64,
    /// Clusters skipped because they failed the filter.
    pub filtered_out: u64,
    /// Every file was read back and matched what was written.
    pub verified: bool,
    /// Things worth knowing (no cluster locations, a tile without base calls, …).
    pub notes: Vec<String>,
}

struct Out {
    path: PathBuf,
    tmp: PathBuf,
    w: flate2::write::GzEncoder<BufWriter<File>>,
    hash: Xxh3,
    records: u64,
    bases: u64,
    read: String,
}

impl Out {
    fn write(&mut self, rec: &[u8]) -> Result<()> {
        self.hash.update(rec);
        self.w
            .write_all(rec)
            .map_err(|e| Error::io(&self.tmp, e))
    }
}

/// Write the run's base calls as FASTQ into `out_dir` (created when missing).
pub fn export_fastq(run: &IlluminaRun, out_dir: &Path, opts: &FastqOptions) -> Result<FastqReport> {
    let calls = run.calls.clone().ok_or_else(|| {
        Error::unsupported(
            FORMAT_ID,
            "FASTQ from a run folder without base calls",
            "This run folder holds InterOp metrics only (no Data/Intensities/BaseCalls); copy the base calls with it to convert it.",
        )
    })?;
    std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;
    openreadout_core::output::refuse_inputs(out_dir, [run.root()])?;
    let fs = run.fs().clone();
    let layout = run.run.layout();
    // reads: (label, first cycle, last cycle, indexed)
    let mut seq_n = 0;
    let mut idx_n = 0;
    let reads: Vec<(String, u32, u32, bool)> = run
        .run
        .reads
        .iter()
        .zip(&layout)
        .map(|(r, (_, a, b))| {
            let label = if r.indexed {
                idx_n += 1;
                format!("I{idx_n}")
            } else {
                seq_n += 1;
                format!("R{seq_n}")
            };
            (label, *a, *b, r.indexed)
        })
        .collect();
    let total = run.run.total_cycles();
    let instrument = run.run.instrument.clone().unwrap_or_default();
    let run_number = run.run.run_number.unwrap_or(0);
    let flowcell = run.run.flowcell.clone().unwrap_or_default();
    let stem = if run.run.run_id.is_empty() {
        "run".to_string()
    } else {
        run.run.run_id.clone()
    };
    let mut report = FastqReport::default();
    let lanes: Vec<u16> = calls
        .tiles
        .keys()
        .copied()
        .filter(|l| opts.lanes.is_empty() || opts.lanes.contains(l))
        .collect();
    if lanes.is_empty() {
        return Err(Error::Usage(format!(
            "no lane to write: the run folder has base calls for lanes {:?}",
            calls.tiles.keys().collect::<Vec<_>>()
        )));
    }
    let window = (WINDOW_BYTES / u64::from(total.max(1))).max(1024);
    for lane in lanes {
        let mut outs: Vec<Out> = Vec::new();
        for (label, ..) in &reads {
            let path = out_dir.join(format!("{stem}_L{lane:03}_{label}.fastq.gz"));
            if path.exists() && !opts.overwrite {
                return Err(Error::Usage(format!(
                    "{} exists; pass overwrite to replace it",
                    path.display()
                )));
            }
            let tmp = openreadout_core::output::temp_path(&path);
            let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            outs.push(Out {
                path,
                tmp,
                w: flate2::write::GzEncoder::new(BufWriter::new(f), flate2::Compression::new(1)),
                hash: Xxh3::new(),
                records: 0,
                bases: 0,
                read: label.clone(),
            });
        }
        let mut lane_clusters = 0u64;
        let tiles: Vec<u32> = calls.tiles[&lane]
            .iter()
            .copied()
            .filter(|t| opts.tiles.is_empty() || opts.tiles.contains(t))
            .collect();
        'tiles: for tile in tiles {
            let filter = calls.read_filter(&fs, lane, tile)?;
            let locs = calls.read_locs(&fs, lane, tile)?;
            if locs.is_none() && report.notes.iter().all(|n| !n.contains("locations")) {
                report
                    .notes
                    .push("no cluster locations: read names carry 0:0 as coordinates".into());
            }
            let n = filter.len() as u64;
            let mut first = 0u64;
            while first < n {
                let count = window.min(n - first);
                let mut cyc: Vec<Vec<u8>> = Vec::with_capacity(total as usize);
                for c in 1..=total {
                    let v = calls.read_cycle(&fs, lane, tile, c, first, count, &filter)?;
                    if (v.len() as u64) < count {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("lane {lane} tile {tile} cycle {c}: {} base calls for {count} clusters", v.len()),
                        ));
                    }
                    cyc.push(v);
                }
                let mut rec = Vec::with_capacity(512);
                for k in 0..count as usize {
                    let ci = first as usize + k;
                    let pass = filter[ci];
                    if !pass && !opts.include_filtered {
                        report.filtered_out += 1;
                        continue;
                    }
                    if opts.max_reads.is_some_and(|m| lane_clusters >= m) {
                        break 'tiles;
                    }
                    lane_clusters += 1;
                    let (x, y) = locs
                        .as_ref()
                        .and_then(|l| l.get(ci))
                        .map_or((0, 0), |(x, y)| (fastq_coordinate(*x), fastq_coordinate(*y)));
                    let mut index = String::new();
                    for (_, a, b, indexed) in &reads {
                        if *indexed {
                            if !index.is_empty() {
                                index.push('+');
                            }
                            for c in *a..=*b {
                                index.push(base(cyc[(c - 1) as usize][k]) as char);
                            }
                        }
                    }
                    let mut seq_read = 0;
                    for (r, (_, a, b, indexed)) in reads.iter().enumerate() {
                        let rn = if *indexed {
                            1
                        } else {
                            seq_read += 1;
                            seq_read
                        };
                        rec.clear();
                        let _ = writeln!(
                            rec,
                            "@{instrument}:{run_number}:{flowcell}:{lane}:{tile}:{x}:{y} {rn}:{}:0:{index}",
                            if pass { 'N' } else { 'Y' }
                        );
                        for c in *a..=*b {
                            rec.push(base(cyc[(c - 1) as usize][k]));
                        }
                        rec.extend_from_slice(b"\n+\n");
                        for c in *a..=*b {
                            rec.push(qual(cyc[(c - 1) as usize][k]));
                        }
                        rec.push(b'\n');
                        let o = &mut outs[r];
                        o.write(&rec)?;
                        o.records += 1;
                        o.bases += u64::from(b - a + 1);
                    }
                }
                first += count;
            }
        }
        report.clusters += lane_clusters;
        for o in outs {
            let Out {
                path,
                tmp,
                w,
                hash,
                records,
                bases,
                read,
            } = o;
            let mut bw = w.finish().map_err(|e| Error::io(&tmp, e))?;
            bw.flush().map_err(|e| Error::io(&tmp, e))?;
            drop(bw);
            let digest = hash.digest128();
            let (n, h) = read_back(&tmp)?;
            if n != records || h != digest {
                let _ = std::fs::remove_file(&tmp);
                return Err(Error::Other(format!(
                    "{}: read back {n} records that differ from the {records} written; nothing was kept",
                    path.display()
                )));
            }
            std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))?;
            report.files.push(FastqFile {
                path,
                lane,
                read,
                records,
                bases,
                xxh3: format!("{digest:032x}"),
            });
        }
    }
    report.verified = true;
    Ok(report)
}

/// The base letter of a base-call byte.
pub fn base(b: u8) -> u8 {
    if b == 0 {
        b'N'
    } else {
        b"ACGT"[(b & 3) as usize]
    }
}

/// The Phred+33 quality character of a base-call byte (a no-call is quality 2).
pub fn qual(b: u8) -> u8 {
    if b == 0 { b'#' } else { 33 + (b >> 2).min(60) }
}

/// Decompress a written file and count and hash its records.
fn read_back(p: &Path) -> Result<(u64, u128)> {
    let f = File::open(p).map_err(|e| Error::io(p, e))?;
    let mut r = BufReader::new(flate2::read::MultiGzDecoder::new(BufReader::new(f)));
    let mut h = Xxh3::new();
    let mut lines = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line).map_err(|e| Error::io(p, e))?;
        if n == 0 {
            break;
        }
        h.update(&line);
        lines += 1;
    }
    Ok((lines / 4, h.digest128()))
}
