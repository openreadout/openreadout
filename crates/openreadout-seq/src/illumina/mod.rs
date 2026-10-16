//! Illumina run folders: the directory an Illumina sequencer writes for one run (`RunInfo.xml`,
//! `RunParameters.xml`, `InterOp/`, `Data/Intensities/BaseCalls/`).
//!
//! `info` reads the two XML files and the InterOp metrics only, never the base calls, so it
//! stays fast on runs of terabytes. The metrics become five tables (run summary per read and
//! lane, metrics per lane and cycle, per tile, the quality-score histogram per cycle, and index
//! counts) and one trace per lane over cycles (`% ≥ Q30`, mean quality, error rate, intensity per
//! channel, base composition). Base calls are decoded by `check`, by the `base calls` table and by
//! the FASTQ export (`fastq.rs`).

pub mod basecalls;
pub mod fastq;
pub mod interop;
pub mod runinfo;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Method, Origin, Quantity, Sample,
};
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo,
    Table, Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result, vocab};
use serde_json::{Value, json};

use crate::table::{Col, MemTable, tidy};
use basecalls::BaseCalls;
use interop::{InterOp, mean_sd};
use runinfo::{RunInfo, RunParameters};

/// Format id of Illumina run folders.
pub const FORMAT_ID: &str = "illumina-run";

/// Largest `RunInfo.xml` / `RunParameters.xml` read.
const MAX_XML: u64 = 16 << 20;

/// InterOp files read, by metric (the `Out` name first, then the older name).
const METRIC_FILES: &[(&str, &[&str])] = &[
    ("tile", &["TileMetricsOut.bin", "TileMetrics.bin"]),
    ("q", &["QMetricsOut.bin", "QMetrics.bin"]),
    ("error", &["ErrorMetricsOut.bin", "ErrorMetrics.bin"]),
    (
        "extraction",
        &["ExtractionMetricsOut.bin", "ExtractionMetrics.bin"],
    ),
    (
        "corrected",
        &["CorrectedIntMetricsOut.bin", "CorrectedIntMetrics.bin"],
    ),
    (
        "extended_tile",
        &["ExtendedTileMetricsOut.bin", "ExtendedTileMetrics.bin"],
    ),
    ("index", &["IndexMetricsOut.bin", "IndexMetrics.bin"]),
];

/// Reader of Illumina run folders.
#[derive(Debug, Default, Clone, Copy)]
pub struct IlluminaRunReader;

/// The run folder a path names: the folder itself, or the folder of a `RunInfo.xml` or
/// `RunParameters.xml` inside it.
fn run_root(fs: &Fs, path: &Path) -> Option<PathBuf> {
    if fs.is_dir(path) {
        return fs
            .is_file(&path.join("RunInfo.xml"))
            .then(|| path.to_path_buf());
    }
    let name = path.file_name()?.to_str()?;
    if name.eq_ignore_ascii_case("RunInfo.xml") || name.eq_ignore_ascii_case("RunParameters.xml") {
        let dir = path.parent()?;
        return fs
            .is_file(&dir.join("RunInfo.xml"))
            .then(|| dir.to_path_buf());
    }
    None
}

impl FormatReader for IlluminaRunReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::ILLUMINA_RUN)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Illumina run folder".into(),
            vendor: "Illumina".into(),
            extensions: vec![],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::ILLUMINA_RUN.confidence,
            known_gaps: vec![
                "InterOp metrics are summarised from tile, quality, error, extraction, corrected-intensity, extended-tile and index files; phasing from empirical-phasing files, event, image, registration and FWHM grid metrics are listed but not decoded".into(),
                "Base calls: BCL (.bcl, .bcl.gz) and CBCL with 2-bit bases and quality bins; NextSeq 500/550 per-lane .bcl.bgzf files are not decoded".into(),
                "FASTQ export writes every read of the run per lane without demultiplexing (index reads are written to their own files); adapter trimming and sample sheets are not applied".into(),
            ],
        }
    }

    fn sniff(&self, _head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(&[], &Input::local(path))
    }

    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        run_root(input.fs(), input.path()).map(|_| Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(IlluminaRun::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

fn read_text(fs: &Fs, p: &Path) -> Result<String> {
    let b = basecalls::read_capped(fs, p, MAX_XML)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// An open Illumina run folder.
#[derive(Debug)]
pub struct IlluminaRun {
    root: PathBuf,
    fs: Fs,
    descriptor: FormatDescriptor,
    /// What `RunInfo.xml` says.
    pub run: RunInfo,
    /// What `RunParameters.xml` says, when the folder has one.
    pub params: Option<RunParameters>,
    params_file: Option<String>,
    /// The InterOp metrics, aggregated.
    pub metrics: InterOp,
    /// The base calls, when the folder holds them.
    pub calls: Option<BaseCalls>,
    tables: Vec<MemTable>,
    /// Lane → per-cycle channels of the quality trace.
    cycle_trace: BTreeMap<u16, Vec<Vec<f64>>>,
    trace_channels: Vec<(String, Option<&'static str>)>,
    vendor: Value,
    bytes_read: u64,
    markers: Vec<String>,
}

impl IlluminaRun {
    /// Open the run folder named by `input` (the folder, or its `RunInfo.xml`).
    pub fn open(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let root = run_root(&fs, input.path()).ok_or_else(|| Error::Corrupt {
            format: FORMAT_ID,
            detail: format!(
                "{} is not an Illumina run folder (no RunInfo.xml)",
                input.path().display()
            ),
            offset: None,
        })?;
        let mut bytes_read = 0u64;
        let ri_text = read_text(&fs, &root.join("RunInfo.xml"))?;
        bytes_read += ri_text.len() as u64;
        let run = runinfo::parse_runinfo(&ri_text)?;
        let mut vendor = serde_json::Map::new();
        vendor.insert(
            "RunInfo".into(),
            openreadout_core::xmljson::xml_to_json(&ri_text).unwrap_or(Value::Null),
        );
        let mut params = None;
        let mut params_file = None;
        for name in ["RunParameters.xml", "runParameters.xml"] {
            let p = root.join(name);
            if fs.is_file(&p) {
                let t = read_text(&fs, &p)?;
                bytes_read += t.len() as u64;
                params = runinfo::parse_runparameters(&t);
                vendor.insert(
                    "RunParameters".into(),
                    openreadout_core::xmljson::xml_to_json(&t).unwrap_or(Value::Null),
                );
                params_file = Some(name.to_string());
                break;
            }
        }
        let layout = run.layout();
        let mut metrics = InterOp::default();
        let interop_dir = root.join("InterOp");
        for (kind, names) in METRIC_FILES {
            let mut parts: Vec<PathBuf> = Vec::new();
            if let Some(p) = names
                .iter()
                .map(|n| interop_dir.join(n))
                .find(|p| fs.is_file(p))
            {
                parts.push(p);
            } else {
                // A run still being written keeps per-cycle copies in InterOp/C<n>.1/.
                let mut cyc: Vec<(u32, PathBuf)> = fs
                    .read_dir(&interop_dir)
                    .map(|rd| {
                        rd.filter_map(std::result::Result::ok)
                            .filter_map(|e| {
                                let n = e.file_name().to_string_lossy().into_owned();
                                let c: u32 =
                                    n.strip_prefix('C')?.strip_suffix(".1")?.parse().ok()?;
                                Some((c, e.path().join(names[0])))
                            })
                            .filter(|(_, p)| fs.is_file(p))
                            .collect()
                    })
                    .unwrap_or_default();
                cyc.sort();
                parts.extend(cyc.into_iter().map(|(_, p)| p));
            }
            for p in parts {
                let b = basecalls::read_capped(&fs, &p, interop::MAX_INTEROP_FILE)?;
                bytes_read += b.len() as u64;
                let name = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                match *kind {
                    "tile" => metrics.add_tile(&name, &b, &layout),
                    "q" => metrics.add_q(&name, &b, &layout),
                    "error" => metrics.add_error(&name, &b, &layout),
                    "extraction" => metrics.add_extraction(&name, &b),
                    "corrected" => metrics.add_corrected(&name, &b),
                    "extended_tile" => metrics.add_extended_tile(&name, &b),
                    _ => metrics.add_index(&name, &b),
                }
            }
        }
        let calls = BaseCalls::discover(&fs, &root);
        let markers = [
            "RTAComplete.txt",
            "CopyComplete.txt",
            "SequenceComplete.txt",
        ]
        .iter()
        .filter(|n| fs.is_file(&root.join(n)))
        .map(|n| (*n).to_string())
        .collect();
        let mut me = IlluminaRun {
            descriptor: IlluminaRunReader.descriptor(),
            root,
            fs,
            run,
            params,
            params_file,
            metrics,
            calls,
            tables: Vec::new(),
            cycle_trace: BTreeMap::new(),
            trace_channels: Vec::new(),
            vendor: Value::Object(vendor),
            bytes_read,
            markers,
        };
        me.build();
        Ok(me)
    }

    /// The run folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file namespace the run folder is read through.
    pub fn fs(&self) -> &Fs {
        &self.fs
    }

    /// Names of the intensity channels: A, C, G, T for four channels, else the image channels
    /// `RunInfo.xml` names, else `channel 1`, `channel 2`, ….
    fn channel_names(&self) -> Vec<String> {
        let n = self.metrics.channels;
        if n == 4 && self.run.image_channels.is_empty() {
            return ["A", "C", "G", "T"]
                .iter()
                .map(|s| (*s).to_string())
                .collect();
        }
        if self.run.image_channels.len() == n {
            return self
                .run
                .image_channels
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
        }
        (1..=n).map(|i| format!("channel {i}")).collect()
    }

    /// Lanes that have metrics or base calls.
    fn lanes(&self) -> Vec<u16> {
        let mut l: Vec<u16> = self
            .metrics
            .tiles
            .keys()
            .map(|(l, _)| *l)
            .chain(self.metrics.cycles.keys().map(|(l, _)| *l))
            .collect();
        if l.is_empty() {
            l = (1..=self.run.lane_count.min(64) as u16).collect();
        }
        l.sort_unstable();
        l.dedup();
        l
    }

    /// Build the tables and the cycle trace from the metrics.
    fn build(&mut self) {
        let layout = self.run.layout();
        let lanes = self.lanes();
        let m = &self.metrics;
        let area = m.tile_area_mm2;
        // ---- run summary: one row per read and lane
        let mut summary = MemTable::new(
            "run summary",
            vec![
                Col::int("read"),
                Col::int("indexed"),
                Col::int("lane"),
                Col::int("tiles"),
                Col::num("density", Some("K/mm²")),
                Col::num("density_sd", Some("K/mm²")),
                Col::num("percent_pf", Some("%")),
                Col::num("percent_pf_sd", Some("%")),
                Col::num("reads", Some("M")),
                Col::num("reads_pf", Some("M")),
                Col::num("percent_q30", Some("%")),
                Col::num("yield", Some("Gbp")),
                Col::num("percent_aligned", Some("%")),
                Col::num("error_rate", Some("%")),
                Col::num("phasing", Some("%")),
                Col::num("prephasing", Some("%")),
                Col::num("percent_occupied", Some("%")),
                Col::num("intensity_cycle_1", None),
            ],
        );
        let mut total_q = (0u64, 0u64);
        let mut total_yield = 0f64;
        let mut lane_reads: BTreeMap<u16, (f64, f64)> = BTreeMap::new();
        for &(read, first, last) in &layout {
            let indexed = self
                .run
                .reads
                .iter()
                .find(|r| r.number == read)
                .is_some_and(|r| r.indexed);
            // The last cycle of a read is left out of its quality and yield totals, as the
            // instrument's own summary does (its quality is not final when the read ends).
            let usable_last = if last > first { last - 1 } else { last };
            for &lane in &lanes {
                let tiles: Vec<&interop::TileTotals> = m
                    .tiles
                    .iter()
                    .filter(|((l, _), _)| *l == lane)
                    .map(|(_, t)| t)
                    .collect();
                let dens = mean_sd(tiles.iter().filter_map(|t| t.density));
                let pf = mean_sd(tiles.iter().filter_map(|t| {
                    let (n, p) = (t.clusters?, t.clusters_pf?);
                    (n > 0.0).then(|| p / n * 100.0)
                }));
                let reads: f64 = tiles.iter().filter_map(|t| t.clusters).sum();
                let reads_pf: f64 = tiles.iter().filter_map(|t| t.clusters_pf).sum();
                lane_reads.insert(lane, (reads, reads_pf));
                let mut q = (0u64, 0u64);
                for c in first..=usable_last {
                    if let Some(ct) = m.cycles.get(&(lane, c)) {
                        for (s, n) in ct.q_hist.iter().enumerate() {
                            q.1 += n;
                            if s >= 30 {
                                q.0 += n;
                            }
                        }
                    }
                }
                total_q.0 += q.0;
                total_q.1 += q.1;
                total_yield += q.1 as f64;
                let aligned = mean_sd(tiles.iter().filter_map(|t| t.aligned.get(&read).copied()));
                // per tile: mean error over the read's cycles that have one, then the mean over tiles
                let err = mean_sd(tiles.iter().filter_map(|t| {
                    t.error
                        .get(&read)
                        .filter(|(_, n)| *n > 0)
                        .map(|(s, n)| s / f64::from(*n))
                }));
                let phasing = mean_sd(tiles.iter().filter_map(|t| t.phasing.get(&read).copied()));
                let prephasing = mean_sd(
                    tiles
                        .iter()
                        .filter_map(|t| t.prephasing.get(&read).copied()),
                );
                let occupied = mean_sd(tiles.iter().filter_map(|t| {
                    let (o, n) = (t.occupied?, t.clusters?);
                    (n > 0.0).then(|| o / n * 100.0)
                }));
                // the first channel's intensity at the read's first cycle, mean over tiles
                let int1 = m.cycles.get(&(lane, first)).and_then(|c| {
                    (c.intensity_tiles > 0 && !c.intensity_sum.is_empty())
                        .then(|| c.intensity_sum[0] / f64::from(c.intensity_tiles))
                });
                let s = &mut summary;
                s.col("read").push(Some(f64::from(read)));
                s.col("indexed").push(Some(f64::from(u8::from(indexed))));
                s.col("lane").push(Some(f64::from(lane)));
                s.col("tiles").push(Some(tiles.len() as f64));
                s.col("density").push(dens.map(|d| d.0 / 1000.0));
                s.col("density_sd").push(dens.map(|d| d.1 / 1000.0));
                s.col("percent_pf").push(pf.map(|p| p.0));
                s.col("percent_pf_sd").push(pf.map(|p| p.1));
                s.col("reads").push(Some(reads / 1e6));
                s.col("reads_pf").push(Some(reads_pf / 1e6));
                s.col("percent_q30")
                    .push((q.1 > 0).then(|| q.0 as f64 / q.1 as f64 * 100.0));
                s.col("yield").push(Some(q.1 as f64 / 1e9));
                s.col("percent_aligned").push(aligned.map(|a| a.0));
                s.col("error_rate").push(err.map(|e| e.0));
                s.col("phasing").push(phasing.map(|p| p.0));
                s.col("prephasing").push(prephasing.map(|p| p.0));
                s.col("percent_occupied").push(occupied.map(|o| o.0));
                s.col("intensity_cycle_1").push(int1);
            }
        }
        summary.extra.insert("kind".into(), json!("run_summary"));
        let (reads_all, pf_all) = lane_reads
            .values()
            .fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
        summary.extra.insert(
            "totals".into(),
            json!({
                "yield_gbp": tidy(total_yield / 1e9, 6),
                "percent_q30": (total_q.1 > 0).then(|| tidy(total_q.0 as f64 / total_q.1 as f64 * 100.0, 5)),
                "reads_m": tidy(reads_all / 1e6, 6),
                "reads_pf_m": tidy(pf_all / 1e6, 6),
                "percent_pf": (reads_all > 0.0).then(|| tidy(pf_all / reads_all * 100.0, 5)),
                "lanes": lanes.len(),
                "cycles": self.run.total_cycles(),
                "read_structure": self.run.read_structure(),
            }),
        );
        if let Some(a) = area {
            summary
                .extra
                .insert("tile_area_mm2".into(), json!(tidy(a, 6)));
        }
        // ---- cycle metrics: one row per lane and cycle
        let channels = self.channel_names();
        let mut cyc = MemTable::new(
            "cycles",
            vec![
                Col::int("lane"),
                Col::int("cycle"),
                Col::int("read"),
                Col::num("percent_q30", Some("%")),
                Col::num("mean_qscore", None),
                Col::num("error_rate", Some("%")),
                Col::num("percent_a", Some("%")),
                Col::num("percent_c", Some("%")),
                Col::num("percent_g", Some("%")),
                Col::num("percent_t", Some("%")),
                Col::num("percent_n", Some("%")),
            ],
        );
        let intensity_names: Vec<String> = channels
            .iter()
            .map(|c| format!("intensity_{}", c.to_ascii_lowercase().replace(' ', "_")))
            .collect();
        for n in &intensity_names {
            cyc.cols.push(Col::num(n.clone(), None));
        }
        let total = self.run.total_cycles();
        let mut trace_channels: Vec<(String, Option<&'static str>)> = vec![
            ("percent_q30".into(), Some("%")),
            ("mean_qscore".into(), None),
            ("error_rate".into(), Some("%")),
        ];
        trace_channels.extend(intensity_names.iter().map(|n| (n.clone(), None)));
        for b in ["a", "c", "g", "t", "n"] {
            trace_channels.push((format!("percent_{b}"), Some("%")));
        }
        let mut cycle_trace = BTreeMap::new();
        for &lane in &lanes {
            let mut tr: Vec<Vec<f64>> = vec![Vec::new(); trace_channels.len()];
            for cycle in 1..=total {
                let ct = m.cycles.get(&(lane, cycle));
                let read = interop::read_of(&layout, cycle);
                let (mut q30, mut qn, mut qs) = (0u64, 0u64, 0f64);
                if let Some(ct) = ct {
                    for (s, n) in ct.q_hist.iter().enumerate() {
                        qn += n;
                        qs += *n as f64 * s as f64;
                        if s >= 30 {
                            q30 += n;
                        }
                    }
                }
                let pq30 = (qn > 0).then(|| q30 as f64 / qn as f64 * 100.0);
                let meanq = (qn > 0).then(|| qs / qn as f64);
                let err = ct.and_then(|c| {
                    (c.error_tiles > 0).then(|| c.error_sum / f64::from(c.error_tiles))
                });
                let calls = ct.map(|c| c.calls).unwrap_or_default();
                let ncalls: u64 = calls.iter().sum();
                let pct = |i: usize| (ncalls > 0).then(|| calls[i] as f64 / ncalls as f64 * 100.0);
                let ints: Vec<Option<f64>> = (0..channels.len())
                    .map(|i| {
                        ct.and_then(|c| {
                            (c.intensity_tiles > 0)
                                .then(|| c.intensity_sum.get(i).copied())
                                .flatten()
                                .map(|s| s / f64::from(c.intensity_tiles))
                        })
                    })
                    .collect();
                if ct.is_none() {
                    // cycles not yet written: in the trace (NaN) but not in the table
                } else {
                    cyc.col("lane").push(Some(f64::from(lane)));
                    cyc.col("cycle").push(Some(f64::from(cycle)));
                    cyc.col("read").push(read.map(f64::from));
                    cyc.col("percent_q30").push(pq30);
                    cyc.col("mean_qscore").push(meanq);
                    cyc.col("error_rate").push(err);
                    cyc.col("percent_a").push(pct(1));
                    cyc.col("percent_c").push(pct(2));
                    cyc.col("percent_g").push(pct(3));
                    cyc.col("percent_t").push(pct(4));
                    cyc.col("percent_n").push(pct(0));
                    for (n, v) in intensity_names.iter().zip(&ints) {
                        cyc.col(n).push(*v);
                    }
                }
                for (k, v) in [pq30, meanq, err]
                    .into_iter()
                    .chain(ints.iter().copied())
                    .chain([pct(1), pct(2), pct(3), pct(4), pct(0)])
                    .enumerate()
                {
                    tr[k].push(v.unwrap_or(f64::NAN));
                }
            }
            cycle_trace.insert(lane, tr);
        }
        cyc.extra.insert("kind".into(), json!("cycle_metrics"));
        // ---- tiles
        let mut tiles = MemTable::new(
            "tiles",
            vec![
                Col::int("lane"),
                Col::int("tile"),
                Col::int("surface"),
                Col::int("swath"),
                Col::num("density", Some("K/mm²")),
                Col::num("density_pf", Some("K/mm²")),
                Col::num("clusters", None),
                Col::num("clusters_pf", None),
                Col::num("percent_pf", Some("%")),
                Col::num("percent_occupied", Some("%")),
                Col::num("percent_q30", Some("%")),
                Col::num("error_rate", Some("%")),
                Col::num("percent_aligned", Some("%")),
            ],
        );
        for ((lane, tile), t) in &m.tiles {
            let digits = tile.to_string();
            let mut ch = digits.chars().filter_map(|c| c.to_digit(10));
            let surface = ch.next();
            let swath = ch.next();
            let q = t
                .q30
                .values()
                .fold((0u64, 0u64), |a, b| (a.0 + b.0, a.1 + b.1));
            let err = {
                let (s, n) = t
                    .error
                    .values()
                    .fold((0.0, 0u32), |a, b| (a.0 + b.0, a.1 + b.1));
                (n > 0).then(|| s / f64::from(n))
            };
            let al = mean_sd(t.aligned.values().copied()).map(|a| a.0);
            tiles.col("lane").push(Some(f64::from(*lane)));
            tiles.col("tile").push(Some(f64::from(*tile)));
            tiles.col("surface").push(surface.map(f64::from));
            tiles.col("swath").push(swath.map(f64::from));
            tiles.col("density").push(t.density.map(|d| d / 1000.0));
            tiles
                .col("density_pf")
                .push(t.density_pf.map(|d| d / 1000.0));
            tiles.col("clusters").push(t.clusters);
            tiles.col("clusters_pf").push(t.clusters_pf);
            tiles
                .col("percent_pf")
                .push(match (t.clusters, t.clusters_pf) {
                    (Some(n), Some(p)) if n > 0.0 => Some(p / n * 100.0),
                    _ => None,
                });
            tiles
                .col("percent_occupied")
                .push(match (t.occupied, t.clusters) {
                    (Some(o), Some(n)) if n > 0.0 => Some(o / n * 100.0),
                    _ => None,
                });
            tiles
                .col("percent_q30")
                .push((q.1 > 0).then(|| q.0 as f64 / q.1 as f64 * 100.0));
            tiles.col("error_rate").push(err);
            tiles.col("percent_aligned").push(al);
        }
        tiles.extra.insert("kind".into(), json!("tile_metrics"));
        // ---- quality-score histogram
        let mut hist = MemTable::new(
            "qscore histogram",
            vec![
                Col::int("lane"),
                Col::int("cycle"),
                Col::int("qscore"),
                Col::num("base_calls", None),
            ],
        );
        for ((lane, cycle), ct) in &m.cycles {
            for (s, n) in ct.q_hist.iter().enumerate() {
                if *n > 0 {
                    hist.col("lane").push(Some(f64::from(*lane)));
                    hist.col("cycle").push(Some(f64::from(*cycle)));
                    hist.col("qscore").push(Some(s as f64));
                    hist.col("base_calls").push(Some(*n as f64));
                }
            }
        }
        hist.extra.insert("kind".into(), json!("qscore_histogram"));
        if !m.q_bins.is_empty() {
            hist.extra.insert(
                "bins".into(),
                json!(
                    m.q_bins
                        .iter()
                        .map(|b| json!({"low": b.low, "high": b.high, "value": b.value}))
                        .collect::<Vec<_>>()
                ),
            );
        }
        // ---- index counts
        let mut index = MemTable::new(
            "index",
            vec![
                Col::int("lane"),
                Col::text("index"),
                Col::text("sample"),
                Col::text("project"),
                Col::num("clusters", None),
                Col::num("percent_of_pf", Some("%")),
            ],
        );
        for ic in &m.index {
            let pf = lane_reads.get(&ic.lane).map_or(0.0, |r| r.1);
            index.col("lane").push(Some(f64::from(ic.lane)));
            index.col("index").push_text(&ic.index);
            index.col("sample").push_text(&ic.sample);
            index.col("project").push_text(&ic.project);
            index.col("clusters").push(Some(ic.clusters as f64));
            index
                .col("percent_of_pf")
                .push((pf > 0.0).then(|| ic.clusters as f64 / pf * 100.0));
        }
        index.extra.insert("kind".into(), json!("index_counts"));
        // ---- base calls (decoded on demand: one row per lane and tile with a filter file)
        let mut bc = MemTable::new(
            "base calls",
            vec![
                Col::int("lane"),
                Col::int("tile"),
                Col::num("clusters", None),
                Col::num("clusters_pf", None),
            ],
        );
        if let Some(c) = &self.calls {
            for (lane, ts) in &c.tiles {
                for t in ts {
                    bc.col("lane").push(Some(f64::from(*lane)));
                    bc.col("tile").push(Some(f64::from(*t)));
                    bc.col("clusters").push(None);
                    bc.col("clusters_pf").push(None);
                }
            }
            bc.extra.insert(
                "storage".into(),
                json!(match c.storage {
                    basecalls::CallStorage::Bcl => "bcl",
                    basecalls::CallStorage::Cbcl => "cbcl",
                }),
            );
            bc.extra.insert(
                "cycles".into(),
                json!(
                    c.cycles
                        .iter()
                        .map(|(l, cs)| json!({"lane": l, "cycles": cs.len()}))
                        .collect::<Vec<_>>()
                ),
            );
        }
        bc.extra.insert("kind".into(), json!("base_calls"));
        bc.extra.insert(
            "note".into(),
            json!(
                "cluster counts are read from the filter files when the table is read, not by info"
            ),
        );
        self.tables = vec![summary, cyc, tiles, hist, index, bc];
        self.cycle_trace = cycle_trace;
        self.trace_channels = trace_channels;
    }

    fn date_iso(&self) -> Option<String> {
        let d = self.run.date.as_deref()?.trim();
        if d.len() == 6 && d.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("20{}-{}-{}", &d[0..2], &d[2..4], &d[4..6]));
        }
        if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]));
        }
        // `6/24/2020 4:30:24 PM`
        let mut it = d.split_whitespace();
        let date = it.next()?;
        let parts: Vec<u32> = date.split('/').filter_map(|p| p.parse().ok()).collect();
        if let [mo, da, yr] = parts[..] {
            let mut out = format!("{yr:04}-{mo:02}-{da:02}");
            if let Some(t) = it.next() {
                let hms: Vec<u32> = t.split(':').filter_map(|p| p.parse().ok()).collect();
                if let [h, mi, s] = hms[..] {
                    let pm = it.next().is_some_and(|x| x.eq_ignore_ascii_case("PM"));
                    let h = match (h, pm) {
                        (12, false) => 0,
                        (12, true) => 12,
                        (h, true) => h + 12,
                        (h, false) => h,
                    };
                    out.push_str(&format!("T{h:02}:{mi:02}:{s:02}"));
                }
            }
            return Some(out);
        }
        // ISO already
        (d.len() >= 10 && d.as_bytes()[4] == b'-').then(|| d.to_string())
    }

    fn model(&self) -> Option<(String, bool)> {
        runinfo::instrument_model(
            self.params.as_ref().and_then(|p| p.application.as_deref()),
            self.params
                .as_ref()
                .and_then(|p| p.instrument_type.as_deref()),
            self.run.instrument.as_deref(),
        )
    }
}

impl Dataset for IlluminaRun {
    fn info(&self) -> Result<FileInfo> {
        let mut traces = Vec::new();
        let lanes: Vec<u16> = self.cycle_trace.keys().copied().collect();
        if !lanes.is_empty() {
            let mut extra = BTreeMap::new();
            let total = self.run.total_cycles();
            extra.insert(
                "axis".into(),
                json!({"quantity": "cycle", "first": 1, "step": 1, "last": total, "size": total}),
            );
            extra.insert("kind".into(), json!("run_quality_by_cycle"));
            extra.insert("sweep_lanes".into(), json!(lanes));
            extra.insert(
                "reads".into(),
                json!(
                    self.run
                        .layout()
                        .iter()
                        .map(|(r, a, b)| json!({"read": r, "first_cycle": a, "last_cycle": b}))
                        .collect::<Vec<_>>()
                ),
            );
            traces.push(TraceInfo {
                index: 0,
                name: Some("run quality by cycle (one sweep per lane)".into()),
                sample_rate_hz: 0.0,
                sample_count: u64::from(total),
                sweep_count: lanes.len() as u32,
                channels: self
                    .trace_channels
                    .iter()
                    .enumerate()
                    .map(|(i, (n, u))| SignalChannelInfo {
                        index: i as u32,
                        name: n.clone(),
                        unit: u.map(str::to_string),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                start_s: None,
                extra,
            });
        }
        let mut notes = Vec::new();
        if self.markers.iter().all(|m| m != "RTAComplete.txt") {
            notes
                .push("no RTAComplete.txt: the run may still be in progress or was stopped".into());
        }
        for (f, why) in &self.metrics.skipped {
            notes.push(format!("{f}: {why}"));
        }
        notes.extend(self.metrics.problems.iter().cloned());
        if self.calls.is_none() {
            notes.push("no base calls under Data/Intensities/BaseCalls (metrics only)".into());
        }
        let mut tables: Vec<_> = self
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| t.info(i as u32))
            .collect();
        if let Some(t) = tables.first_mut() {
            t.extra.insert("run_id".into(), json!(self.run.run_id));
            if let Some(f) = &self.run.flowcell {
                t.extra.insert("flowcell".into(), json!(f));
            }
            if let Some(i) = &self.run.instrument {
                t.extra.insert("instrument".into(), json!(i));
            }
            if let Some((m, _)) = self.model() {
                t.extra.insert("instrument_model".into(), json!(m));
            }
            t.extra.insert(
                "reads".into(),
                json!(
                    self.run
                        .reads
                        .iter()
                        .map(
                            |r| json!({"read": r.number, "cycles": r.cycles, "indexed": r.indexed})
                        )
                        .collect::<Vec<_>>()
                ),
            );
            if let Some(p) = &self.params {
                for (k, v) in [
                    ("experiment", &p.experiment),
                    ("control_software", &p.application),
                    ("control_software_version", &p.application_version),
                    ("rta_version", &p.rta_version),
                    ("flowcell_mode", &p.flowcell_mode),
                    ("chemistry", &p.chemistry),
                ] {
                    if let Some(v) = v {
                        t.extra.insert(k.into(), json!(v));
                    }
                }
            }
            t.extra
                .insert("interop_versions".into(), json!(self.metrics.versions));
            t.extra
                .insert("completion_markers".into(), json!(self.markers));
        }
        Ok(FileInfo {
            path: self.root.display().to_string(),
            size_bytes: self.bytes_read,
            format: self.descriptor.clone(),
            format_version: self.run.version.clone(),
            images: vec![],
            tables,
            spectra: vec![],
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "tables[0]",
            "tables[1]",
            "tables[2]",
            "tables[3]",
            "tables[4]",
        ] {
            p.insert(k.into(), Source::VendorImpl);
        }
        p.insert("tables[5]".into(), Source::PriorArt);
        p.insert("traces[0]".into(), Source::VendorImpl);
        p.insert(
            "tables[0].extra.instrument_model".into(),
            match self.model() {
                Some((_, true)) => Source::VendorImpl,
                _ => Source::Inferred,
            },
        );
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut e = vec![LsEntry {
            kind: "run-info".into(),
            name: "RunInfo.xml".into(),
            offset: None,
            size: None,
            image: None,
            details: json!({"run_id": self.run.run_id, "read_structure": self.run.read_structure(), "lanes": self.run.lane_count}),
        }];
        if let Some(n) = &self.params_file {
            e.push(LsEntry {
                kind: "run-parameters".into(),
                name: n.clone(),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        for (f, v) in &self.metrics.versions {
            e.push(LsEntry {
                kind: "interop".into(),
                name: f.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"version": v, "decoded": !self.metrics.skipped.contains_key(f)}),
            });
        }
        if let Some(c) = &self.calls {
            for (lane, cs) in &c.cycles {
                e.push(LsEntry {
                    kind: "basecalls".into(),
                    name: format!("Data/Intensities/BaseCalls/L{lane:03}"),
                    offset: None,
                    size: None,
                    image: None,
                    details: json!({"cycles": cs.len(), "tiles": c.tiles.get(lane).map_or(0, Vec::len), "storage": format!("{:?}", c.storage).to_ascii_lowercase()}),
                });
            }
        }
        Ok(e)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "an Illumina run folder holds metrics tables and traces, not images (see info → tables[] and traces[])".into(),
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let n = self.tables.len() as u32;
        if index >= n {
            return Err(Error::Usage(format!(
                "table {index} does not exist; this run folder has tables 0..{} (see info → tables[])",
                n - 1
            )));
        }
        if index == 5 {
            // base calls: fill the counts of the rows asked for from the filter files
            let t = self.tables[5].slice(5, first_row, max_rows);
            let mut out = t.clone();
            if let Some(c) = &self.calls {
                for r in 0..out.columns[0].len() {
                    let lane = out.columns[0][r] as u16;
                    let tile = out.columns[1][r] as u32;
                    let f = c.read_filter(&self.fs, lane, tile)?;
                    out.columns[2][r] = f.len() as f64;
                    out.columns[3][r] = f.iter().filter(|x| **x).count() as f64;
                }
            }
            return Ok(out);
        }
        Ok(self.tables[index as usize].slice(index, first_row, max_rows))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index != 0 || self.cycle_trace.is_empty() {
            return Err(Error::Usage(format!(
                "trace {index} does not exist; trace 0 is the run quality by cycle (one sweep per lane)"
            )));
        }
        let lane = self
            .cycle_trace
            .keys()
            .nth(sweep as usize)
            .copied()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "sweep {sweep} does not exist; trace 0 has one sweep per lane ({} lanes)",
                    self.cycle_trace.len()
                ))
            })?;
        let chans = &self.cycle_trace[&lane];
        let n = chans.first().map_or(0, Vec::len) as u64;
        let a = first_sample.min(n) as usize;
        let b = first_sample.saturating_add(max_samples).min(n) as usize;
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: chans.iter().map(|c| c[a..b].to_vec()).collect(),
        })
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.root.display().to_string(), FORMAT_ID);
        r.performed("RunInfo.xml parsed; reads and flow-cell layout read");
        r.performed("InterOp files decoded and every record bounds-checked");
        if self.markers.iter().all(|m| m != "RTAComplete.txt") {
            r.push(Finding::warning(
                "run_incomplete",
                "no RTAComplete.txt: the run may still be in progress or was stopped",
            ));
        }
        for p in &self.metrics.problems {
            r.push(Finding::warning("interop_truncated", p.clone()));
        }
        for (f, why) in &self.metrics.skipped {
            r.push(Finding::info("interop_not_decoded", format!("{f}: {why}")));
        }
        if let Some(c) = &self.calls {
            r.performed("base-call directories listed: every lane has every cycle of RunInfo.xml");
            let total = self.run.total_cycles();
            for (lane, cs) in &c.cycles {
                let have: std::collections::HashSet<u32> = cs.iter().copied().collect();
                let missing: Vec<u32> = (1..=total).filter(|c| !have.contains(c)).collect();
                if !missing.is_empty() {
                    r.push(Finding::error(
                        "cycles_missing",
                        format!(
                            "lane {lane}: {} of {total} cycles have no base calls (first missing: {})",
                            missing.len(),
                            missing[0]
                        ),
                    ));
                }
            }
        }
        Ok(r)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        let Some(c) = self.calls.clone() else {
            return Ok(r);
        };
        r.performed("every tile's filter file read; every cycle's base calls of every tile decoded and counted against the filter");
        let total = self.run.total_cycles();
        let mut tiles_done = 0u32;
        for (lane, ts) in &c.tiles {
            for &tile in ts {
                let filter = match c.read_filter(&self.fs, *lane, tile) {
                    Ok(f) => f,
                    Err(e) => {
                        r.push(Finding::error("filter_unreadable", e.to_string()));
                        continue;
                    }
                };
                let n = filter.len() as u64;
                for cycle in 1..=total {
                    if c.cycle_file(&self.fs, *lane, tile, cycle).is_none() {
                        continue; // reported by check_headers
                    }
                    match c.read_cycle(&self.fs, *lane, tile, cycle, 0, n, &filter) {
                        Ok(v) if v.len() as u64 == n => {}
                        Ok(v) => r.push(Finding::error(
                            "cluster_count_mismatch",
                            format!(
                                "lane {lane} tile {tile} cycle {cycle}: {} base calls for {n} clusters",
                                v.len()
                            ),
                        )),
                        Err(e) => r.push(Finding::error("basecalls_unreadable", e.to_string())),
                    }
                }
                tiles_done += 1;
            }
        }
        r.push(Finding::info(
            "basecalls_decoded",
            format!("{tiles_done} tiles × {total} cycles decoded"),
        ));
        Ok(r)
    }

    fn experiment(&self) -> Option<Experiment> {
        let mut e = Experiment::default();
        let origin = |s: Source, from: &str| Origin {
            source: s,
            from: from.to_string(),
        };
        let mut inst = ExperimentInstrument {
            vendor: Some("Illumina".into()),
            serial: self.run.instrument.clone(),
            kind: vocab::term("OBI:0400103"),
            ..ExperimentInstrument::default()
        };
        e.provenance.insert(
            "instrument.serial".into(),
            origin(Source::VendorImpl, "RunInfo.xml Run/Instrument"),
        );
        if let Some((m, from_file)) = self.model() {
            inst.model = Some(m);
            e.provenance.insert(
                "instrument.model".into(),
                if from_file {
                    origin(Source::VendorImpl, "RunParameters.xml ApplicationName")
                } else {
                    origin(Source::Inferred, "RunInfo.xml Run/Instrument (id prefix)")
                },
            );
        }
        if let Some(p) = &self.params {
            inst.software.clone_from(&p.application);
            inst.software_version.clone_from(&p.application_version);
        }
        e.instrument = Some(inst);
        let mut method = Method {
            name: self.params.as_ref().and_then(|p| p.experiment.clone()),
            technique: vocab::term("OBI:0000734"),
            assay: vocab::term("OBI:0000626"),
            ..Method::default()
        };
        method.parameters.insert(
            "read_structure".into(),
            Quantity::plain(self.run.read_structure()),
        );
        method
            .parameters
            .insert("cycles".into(), Quantity::plain(self.run.total_cycles()));
        method
            .parameters
            .insert("lanes".into(), Quantity::plain(self.run.lane_count));
        if let Some(f) = &self.run.flowcell {
            method
                .parameters
                .insert("flowcell".into(), Quantity::plain(f.clone()));
        }
        if let Some(p) = &self.params {
            if let Some(v) = &p.rta_version {
                method
                    .parameters
                    .insert("rta_version".into(), Quantity::plain(v.clone()));
            }
            if let Some(v) = &p.flowcell_mode {
                method
                    .parameters
                    .insert("flowcell_mode".into(), Quantity::plain(v.clone()));
            }
            if let Some(v) = &p.chemistry {
                method
                    .parameters
                    .insert("chemistry".into(), Quantity::plain(v.clone()));
            }
        }
        e.method = Some(method);
        e.acquisition = Some(Acquisition {
            started_at: self.date_iso(),
            operator: self.params.as_ref().and_then(|p| p.operator.clone()),
            ..Acquisition::default()
        });
        e.provenance.insert(
            "acquisition.started_at".into(),
            origin(Source::VendorImpl, "RunInfo.xml Run/Date"),
        );
        if let Some(lib) = self.params.as_ref().and_then(|p| p.library_tube.clone()) {
            e.sample = Some(Sample {
                barcode: Some(lib),
                source_field: Some("RunParameters.xml LibraryTubeSerialBarcode".into()),
                ..Sample::default()
            });
        }
        Some(e)
    }
}
