//! Oxford Nanopore raw signal: POD5 (`pod5.rs`) and FAST5 (`fast5.rs`) behind one dataset.
//!
//! Every read is one sweep of trace 0 (channel `current`, pA: `(adc + offset) × scale` with the
//! read's calibration), so `trace --sweep N` is read N's squiggle. Table 0 lists the reads
//! (id, channel, well, start, duration, samples, open-pore level, end reason, calibration) in the
//! same order. `info` reads metadata only; the signal is decoded per read on demand.

pub mod fast5;
pub mod ipc;
pub mod pod5;
pub mod vbz;

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
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result, vocab};
use serde_json::{Value, json};

use crate::table::{Col, MemTable, tidy};

/// One read's metadata.
#[derive(Debug, Clone, Default)]
pub struct ReadMeta {
    /// Read id (UUID).
    pub read_id: String,
    /// Channel (1-based).
    pub channel: Option<u32>,
    /// Well (mux), 1-based.
    pub well: Option<u32>,
    /// Read number on its channel.
    pub read_number: Option<u64>,
    /// Samples since the acquisition started, at the read's start.
    pub start_sample: Option<u64>,
    /// Samples in the read.
    pub num_samples: Option<u64>,
    /// Current level before the read (pA).
    pub median_before: Option<f64>,
    /// Open-pore level (pA).
    pub open_pore_level: Option<f64>,
    /// Why the read ended (name in POD5, numeric code in FAST5).
    pub end_reason: Option<String>,
    /// The read was ended forcibly (mux change, unblock).
    pub end_reason_forced: Option<bool>,
    /// Pore type.
    pub pore_type: Option<String>,
    /// MinKNOW events in the read.
    pub num_events: Option<u64>,
    /// Calibration: pA = (adc + offset) × scale.
    pub offset: f64,
    /// See `offset`.
    pub scale: f64,
    /// Index of the read's run in the run list.
    pub run: Option<usize>,
    /// The run's acquisition id as the read names it.
    pub run_name: Option<String>,
    /// Samples per second.
    pub sample_rate: Option<f64>,
}

/// One run (acquisition).
#[derive(Debug, Clone, Default)]
pub struct RunMeta {
    /// Acquisition id.
    pub acquisition_id: String,
    /// Acquisition start, ms since 1970.
    pub started_ms: Option<i64>,
    /// Protocol start, ms since 1970.
    pub protocol_started_ms: Option<i64>,
    /// Experiment (protocol group) name.
    pub experiment: Option<String>,
    /// Flow-cell id.
    pub flow_cell_id: Option<String>,
    /// Flow-cell product code (`FLO-PRO114M`).
    pub flow_cell_product: Option<String>,
    /// Protocol name.
    pub protocol: Option<String>,
    /// Protocol run id.
    pub protocol_run_id: Option<String>,
    /// Sample id.
    pub sample_id: Option<String>,
    /// Samples per second.
    pub sample_rate: Option<f64>,
    /// Sequencing kit.
    pub sequencing_kit: Option<String>,
    /// Sequencer position.
    pub position: Option<String>,
    /// Sequencer position type (`PromethION`, `MinION Mk1B`).
    pub position_type: Option<String>,
    /// Writing software.
    pub software: Option<String>,
    /// System name.
    pub system_name: Option<String>,
    /// System type.
    pub system_type: Option<String>,
    /// ADC range.
    pub adc_min: Option<i64>,
    /// See `adc_min`.
    pub adc_max: Option<i64>,
    /// Context tags.
    pub context_tags: Vec<(String, String)>,
    /// Tracking id fields.
    pub tracking_id: Vec<(String, String)>,
}

/// Decodes one read's raw ADC samples.
pub trait SignalSource
where
    Self: Send + std::fmt::Debug,
{
    /// The raw samples of read `read` (index into the reads list).
    fn raw(&mut self, read: usize) -> Result<Vec<i16>>;
}

/// An open POD5 or FAST5 file.
#[derive(Debug)]
pub struct NanoporeDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    version: Option<String>,
    /// Reads.
    pub reads: Vec<ReadMeta>,
    /// Runs.
    pub runs: Vec<RunMeta>,
    signal: Box<dyn SignalSource>,
    table: MemTable,
    extra: BTreeMap<String, Value>,
    vendor: Value,
    cached: Option<(usize, Vec<i16>)>,
}

fn iso_ms(ms: i64) -> String {
    openreadout_core::time::unix_to_iso8601(ms.div_euclid(1000), ms.rem_euclid(1000) as u32)
}

impl NanoporeDataset {
    fn new(
        descriptor: FormatDescriptor,
        path: &Path,
        size: u64,
        version: Option<String>,
        reads: Vec<ReadMeta>,
        runs: Vec<RunMeta>,
        signal: Box<dyn SignalSource>,
        mut extra: BTreeMap<String, Value>,
        mut vendor: serde_json::Map<String, Value>,
    ) -> Self {
        let rate_of = |r: &ReadMeta| r.sample_rate.filter(|x| *x > 0.0);
        let mut t = MemTable::new(
            "reads",
            vec![
                Col::text("read_id"),
                Col::int("channel"),
                Col::int("well"),
                Col::int("read_number"),
                Col::num("start", Some("s")),
                Col::num("duration", Some("s")),
                Col::int("samples"),
                Col::num("median_before", Some("pA")),
                Col::num("open_pore_level", Some("pA")),
                Col::text("end_reason"),
                Col::int("end_reason_forced"),
                Col::num("calibration_offset", None),
                Col::num("calibration_scale", Some("pA")),
            ],
        );
        let mut durations = Vec::new();
        for r in &reads {
            let rate = rate_of(r);
            t.col("read_id").push_text(&r.read_id);
            t.col("channel").push(r.channel.map(f64::from));
            t.col("well").push(r.well.map(f64::from));
            t.col("read_number").push(r.read_number.map(|x| x as f64));
            t.col("start")
                .push(r.start_sample.zip(rate).map(|(s, h)| s as f64 / h));
            let d = r.num_samples.zip(rate).map(|(s, h)| s as f64 / h);
            if let Some(d) = d {
                durations.push(d);
            }
            t.col("duration").push(d);
            t.col("samples").push(r.num_samples.map(|x| x as f64));
            t.col("median_before").push(r.median_before);
            t.col("open_pore_level").push(r.open_pore_level);
            t.col("end_reason")
                .push_text(r.end_reason.as_deref().unwrap_or(""));
            t.col("end_reason_forced")
                .push(r.end_reason_forced.map(|b| f64::from(u8::from(b))));
            t.col("calibration_offset").push(Some(r.offset));
            t.col("calibration_scale").push(Some(r.scale));
        }
        t.extra.insert("kind".into(), json!("nanopore_reads"));
        let total_samples: u64 = reads.iter().filter_map(|r| r.num_samples).sum();
        durations.sort_by(f64::total_cmp);
        let total_s: f64 = durations.iter().sum();
        let n50 = {
            let mut acc = 0.0;
            let mut out = None;
            for d in durations.iter().rev() {
                acc += d;
                if acc >= total_s / 2.0 {
                    out = Some(*d);
                    break;
                }
            }
            out
        };
        t.extra.insert(
            "summary".into(),
            json!({
                "reads": reads.len(),
                "samples": total_samples,
                "signal_s": tidy(total_s, 6),
                "duration_median_s": durations.get(durations.len() / 2).map(|d| tidy(*d, 4)),
                "duration_max_s": durations.last().map(|d| tidy(*d, 4)),
                "duration_n50_s": n50.map(|d| tidy(d, 4)),
                "channels": reads.iter().filter_map(|r| r.channel).collect::<std::collections::BTreeSet<_>>().len(),
            }),
        );
        if let Some(run) = runs.first() {
            for (k, v) in [
                ("flow_cell_id", &run.flow_cell_id),
                ("flow_cell_product", &run.flow_cell_product),
                ("sequencing_kit", &run.sequencing_kit),
                ("sample_id", &run.sample_id),
                ("experiment", &run.experiment),
                ("protocol", &run.protocol),
                ("software", &run.software),
                ("position", &run.position),
                ("position_type", &run.position_type),
                ("system_type", &run.system_type),
            ] {
                if let Some(v) = v {
                    t.extra.insert(k.into(), json!(v));
                }
            }
            if let Some(ms) = run.started_ms {
                t.extra
                    .insert("acquisition_start".into(), json!(iso_ms(ms)));
            }
        }
        if runs.len() > 1 {
            t.extra.insert("runs".into(), json!(runs.len()));
        }
        let rate = reads.iter().find_map(rate_of);
        extra.insert("kind".into(), json!("nanopore_signal"));
        if reads.iter().filter_map(rate_of).any(|r| Some(r) != rate) {
            extra.insert(
                "note".into(),
                json!("reads have different sample rates; durations use each read's own"),
            );
        }
        vendor.insert(
            "runs".into(),
            Value::Array(
                runs.iter()
                    .map(|r| {
                        json!({
                            "acquisition_id": r.acquisition_id,
                            "context_tags": r.context_tags.iter().cloned().collect::<BTreeMap<_, _>>(),
                            "tracking_id": r.tracking_id.iter().cloned().collect::<BTreeMap<_, _>>(),
                        })
                    })
                    .collect(),
            ),
        );
        NanoporeDataset {
            descriptor,
            path: path.to_path_buf(),
            size,
            version,
            reads,
            runs,
            signal,
            table: t,
            extra,
            vendor: Value::Object(vendor),
            cached: None,
        }
    }

    fn raw(&mut self, read: usize) -> Result<&[i16]> {
        if self.cached.as_ref().is_none_or(|c| c.0 != read) {
            let v = self.signal.raw(read)?;
            self.cached = Some((read, v));
        }
        Ok(self.cached.as_ref().map_or(&[][..], |c| c.1.as_slice()))
    }

    fn rate(&self) -> f64 {
        self.reads
            .iter()
            .find_map(|r| r.sample_rate.filter(|x| *x > 0.0))
            .or_else(|| self.runs.iter().find_map(|r| r.sample_rate))
            .unwrap_or(0.0)
    }
}

impl Dataset for NanoporeDataset {
    fn info(&self) -> Result<FileInfo> {
        let counts: Vec<u64> = self
            .reads
            .iter()
            .map(|r| r.num_samples.unwrap_or(0))
            .collect();
        let mut extra = self.extra.clone();
        extra.insert("sweep_sample_counts".into(), json!(counts));
        extra.insert(
            "sweeps".into(),
            json!("one sweep per read, in the order of table 0 (reads)"),
        );
        let traces = if self.reads.is_empty() {
            vec![]
        } else {
            vec![TraceInfo {
                index: 0,
                name: Some("raw signal (one sweep per read)".into()),
                sample_rate_hz: self.rate(),
                sample_count: counts.iter().copied().max().unwrap_or(0),
                sweep_count: self.reads.len().min(u32::MAX as usize) as u32,
                channels: vec![SignalChannelInfo {
                    index: 0,
                    name: "current".into(),
                    unit: Some("pA".into()),
                    dtype: "int16".into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: BTreeMap::from([(
                        "calibration".into(),
                        json!(
                            "per read: pA = (adc + calibration_offset) × calibration_scale (table 0)"
                        ),
                    )]),
                }],
                start_s: Some(0.0),
                extra,
            }]
        };
        let mut notes = Vec::new();
        if self.reads.is_empty() {
            notes.push("the file holds no reads".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: self.version.clone(),
            images: vec![],
            tables: vec![self.table.info(0)],
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
        let src = if self.descriptor.id == pod5::FORMAT_ID {
            Source::VendorImpl
        } else {
            Source::Inferred
        };
        ProvenanceMap::from([
            ("traces[0]".into(), src),
            ("tables[0]".into(), src),
            ("traces[0].channels[0].unit".into(), src),
        ])
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .reads
            .iter()
            .enumerate()
            .map(|(i, r)| LsEntry {
                kind: "read".into(),
                name: r.read_id.clone(),
                offset: None,
                size: r.num_samples,
                image: None,
                details: json!({"sweep": i, "channel": r.channel, "well": r.well}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a nanopore file holds signal traces and a reads table, not images".into(),
        ))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} does not exist; table 0 lists the reads"
            )));
        }
        Ok(self.table.slice(0, first_row, max_rows))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace {index} does not exist; trace 0 holds every read's signal (one sweep per read)"
            )));
        }
        let read = sweep as usize;
        let n = self.reads.len();
        let meta = self.reads.get(read).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} does not exist: the file has {n} reads (sweeps 0..{})",
                n.saturating_sub(1)
            ))
        })?;
        let raw = self.raw(read)?;
        let len = raw.len() as u64;
        let a = first_sample.min(len) as usize;
        let b = first_sample.saturating_add(max_samples).min(len) as usize;
        let v: Vec<f64> = raw[a..b]
            .iter()
            .map(|x| (f64::from(*x) + meta.offset) * meta.scale)
            .collect();
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: vec![v],
        })
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("container and metadata read; every read listed with its run");
        let orphans = self.reads.iter().filter(|x| x.run.is_none()).count();
        if orphans > 0 && !self.runs.is_empty() {
            r.push(Finding::warning(
                "reads_without_run",
                format!("{orphans} reads name a run the file does not describe"),
            ));
        }
        Ok(r)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every read's signal decoded; sample counts compared with the reads table");
        let mut bad = 0usize;
        for i in 0..self.reads.len() {
            let want = self.reads[i].num_samples;
            match self.signal.raw(i) {
                Ok(v) => {
                    if want.is_some_and(|n| n != v.len() as u64) {
                        bad += 1;
                        if bad <= 10 {
                            r.push(Finding::error(
                                "sample_count_mismatch",
                                format!(
                                    "read {} ({}): {} samples decoded, {} listed",
                                    i,
                                    self.reads[i].read_id,
                                    v.len(),
                                    want.unwrap_or(0)
                                ),
                            ));
                        }
                    }
                }
                Err(e) => {
                    bad += 1;
                    if bad <= 10 {
                        r.push(Finding::error(
                            "signal_unreadable",
                            format!("read {i}: {e}"),
                        ));
                    }
                }
            }
        }
        if bad > 10 {
            r.push(Finding::error(
                "signal_unreadable",
                format!("{bad} reads in all could not be decoded or disagree with the reads table"),
            ));
        }
        r.push(Finding::info(
            "reads_decoded",
            format!("{} reads decoded", self.reads.len() - bad),
        ));
        Ok(r)
    }

    fn experiment(&self) -> Option<Experiment> {
        let run = self.runs.first()?;
        let mut e = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::VendorImpl,
            from: from.to_string(),
        };
        let model = run
            .system_type
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| run.position_type.clone());
        e.instrument = Some(ExperimentInstrument {
            vendor: Some("Oxford Nanopore Technologies".into()),
            model,
            serial: run.system_name.clone().or_else(|| run.position.clone()),
            software: run.software.clone(),
            kind: vocab::term("OBI:0400103"),
            ..ExperimentInstrument::default()
        });
        e.provenance
            .insert("instrument.model".into(), origin("run info system_type"));
        let mut method = Method {
            name: run.protocol.clone(),
            technique: vocab::term("OBI:0000626"),
            assay: vocab::term("OBI:0000626"),
            ..Method::default()
        };
        if let Some(r) = run.sample_rate {
            method
                .parameters
                .insert("sample_rate".into(), Quantity::number(r, "Hz"));
        }
        for (k, v) in [
            ("flow_cell_product", &run.flow_cell_product),
            ("flow_cell_id", &run.flow_cell_id),
            ("sequencing_kit", &run.sequencing_kit),
        ] {
            if let Some(v) = v {
                method
                    .parameters
                    .insert(k.into(), Quantity::plain(v.clone()));
            }
        }
        e.method = Some(method);
        e.acquisition = Some(Acquisition {
            started_at: run.started_ms.map(iso_ms).or_else(|| {
                run.tracking_id
                    .iter()
                    .find(|(k, _)| k == "exp_start_time")
                    .map(|(_, v)| v.clone())
            }),
            ..Acquisition::default()
        });
        if let Some(s) = run.sample_id.clone().filter(|s| !s.is_empty()) {
            e.sample = Some(Sample {
                id: Some(s),
                source_field: Some("run info sample_id".into()),
                ..Sample::default()
            });
        }
        Some(e)
    }
}

/// Reader of POD5 files.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pod5Reader;

impl FormatReader for Pod5Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::POD5)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: pod5::FORMAT_ID.into(),
            name: "Oxford Nanopore POD5".into(),
            vendor: "Oxford Nanopore Technologies".into(),
            extensions: vec!["pod5".into()],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::POD5.confidence,
            known_gaps: vec![
                "Base calls are not in POD5; basecalled reads come as FASTQ or BAM (read those files)".into(),
                "The read-id index tables are not used; reads are listed in the order of the reads table".into(),
                "Files written by pod5 before 0.1 (dictionaries of structs in the reads table) are refused (exit 6)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if pod5::looks_like(head) {
            return Some(Detection {
                format_id: pod5::FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["pod5"]).then_some(Detection {
            format_id: pod5::FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("POD5 extension but no POD5 signature".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let p = pod5::open(input)?;
        let mut extra = BTreeMap::new();
        extra.insert(
            "signal_compression".into(),
            json!(if p.signal_is_vbz() { "vbz" } else { "none" }),
        );
        let mut vendor = serde_json::Map::new();
        vendor.insert(
            "footer".into(),
            json!({
                "file_identifier": p.footer.file_identifier,
                "software": p.footer.software,
                "pod5_version": p.footer.pod5_version,
                "contents": p.footer.contents.iter().map(|c| json!({"offset": c.offset, "length": c.length, "content_type": c.content})).collect::<Vec<_>>(),
            }),
        );
        let version = Some(p.footer.pod5_version.clone());
        let mut runs = p.runs;
        if let Some(r) = runs.first_mut()
            && r.software.is_none()
        {
            r.software = Some(p.footer.software.clone());
        }
        Ok(Box::new(NanoporeDataset::new(
            self.descriptor(),
            input.path(),
            p.size,
            version,
            p.reads,
            runs,
            Box::new(p.signal),
            extra,
            vendor,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Reader of FAST5 files.
#[derive(Debug, Default, Clone, Copy)]
pub struct Fast5Reader;

impl FormatReader for Fast5Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::FAST5)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: fast5::FORMAT_ID.into(),
            name: "Oxford Nanopore FAST5".into(),
            vendor: "Oxford Nanopore Technologies".into(),
            extensions: vec!["fast5".into()],
            family: "sequencing".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::FAST5.confidence,
            known_gaps: vec![
                "Signal compressed with deflate or VBZ (filter 32020, version 1) is read; other HDF5 filters are refused".into(),
                "Base calls stored in Analyses groups are counted, not read; the end reason of FAST5 2.x is reported as its stored code".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (openreadout_hdf5::h5util::looks_like_hdf5(head) && has_extension(path, &["fast5"]))
            .then_some(Detection {
                format_id: fast5::FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let f = fast5::open(input)?;
        let mut extra = BTreeMap::new();
        extra.insert("signal_compression".into(), json!(f.compression));
        extra.insert(
            "layout".into(),
            json!(
                if f.reads.len() == 1 && f.version.as_deref().is_none_or(|v| v.starts_with('0')) {
                    "single-read"
                } else {
                    "multi-read"
                }
            ),
        );
        if f.basecalled > 0 {
            extra.insert("basecalled_reads".into(), json!(f.basecalled));
        }
        let version = f.version.clone();
        Ok(Box::new(NanoporeDataset::new(
            self.descriptor(),
            input.path(),
            f.size,
            version,
            f.reads,
            f.runs,
            Box::new(f.signal),
            extra,
            serde_json::Map::new(),
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
