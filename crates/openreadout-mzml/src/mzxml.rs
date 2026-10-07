//! mzXML 2.x / 3.x (ISB / Seattle Proteome Center) — the dataset.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, InstrumentInfo, LsEntry, SpectraInfo,
    Spectrum,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Plane, Result};

use crate::binary::{ArrayEncoding, ArrayError, Compression, ValueType, decode_array};
use crate::scan::{
    Located, WrittenIndex, read_written_index, trailer_offset, verify_offsets, walk,
};
use crate::xml::{Node, element_at, element_in};

pub(crate) const FMT: &str = "mzxml";

/// Children of `msRun` that make up the header.
const HEADER_TAGS: &[&str] = &[
    "parentFile",
    "msInstrument",
    "dataProcessing",
    "separation",
    "spotting",
];

const LS_SCANS: usize = 1000;

/// Scan metadata for `info`.
#[derive(Debug, Clone, Default)]
struct Summary {
    ms_level: u32,
    rt_s: Option<f64>,
    polarity: String,
    centroided: bool,
    points: u64,
}

/// An open mzXML file.
#[derive(Debug)]
pub struct MzxmlDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file_len: u64,
    version: Option<String>,
    root_attrs: Vec<(String, String)>,
    header: Vec<Node>,
    run: Node,
    scans: Vec<Located>,
    by_index: bool,
    written_index: Option<WrittenIndex>,
    index_problem: Option<String>,
    complete: bool,
    notes: Vec<String>,
    summaries: OnceLock<std::result::Result<Vec<Summary>, String>>,
}

fn walk_mzxml(fs: &Fs, path: &Path, stop_at_data: bool) -> Result<crate::scan::Walk> {
    walk(
        fs,
        path,
        &["mzXML"],
        "msRun",
        HEADER_TAGS,
        &[],
        &["scan"],
        stop_at_data,
    )
}

/// `xs:duration` such as `PT353.43S`, `PT5M3.2S`, `P0DT1H` → seconds.
pub fn parse_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let neg = s.starts_with('-');
    let s = s.trim_start_matches('-').strip_prefix('P')?;
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };
    let mut total = 0.0;
    let mut num = String::new();
    for c in date.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let v: f64 = num.parse().ok()?;
            num.clear();
            total += v * match c {
                'D' => 86_400.0,
                'Y' | 'M' => return None, // calendar units have no fixed length
                _ => return None,
            };
        }
    }
    for c in time.chars() {
        if c.is_ascii_digit() || c == '.' || c == 'E' || c == 'e' || c == '+' || c == '-' {
            num.push(c);
        } else {
            let v: f64 = num.parse().ok()?;
            num.clear();
            total += v * match c {
                'H' => 3600.0,
                'M' => 60.0,
                'S' => 1.0,
                _ => return None,
            };
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

fn attr_f64(n: &Node, k: &str) -> Option<f64> {
    n.attr(k)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

impl MzxmlDataset {
    /// `deep`: also verify the index offsets, scan the whole file and decode every scan
    /// (`check`); without it only completeness, the index problem found at open and the declared
    /// scan count (`check --headers-only`).
    fn run_check(&self, deep: bool) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FMT);
        if !self.complete {
            rep.push(Finding::error(
                "truncated",
                "the file ends before </mzXML> (interrupted write or copy)",
            ));
        }
        if let Some(p) = &self.index_problem {
            rep.push(Finding::error("index_mismatch", p.clone()));
        }
        if let Some(ix) = self.written_index.as_ref().filter(|_| deep) {
            rep.performed("validated every index offset against the <scan num> it names");
            let list = ix.list("scan").unwrap_or_default();
            let bad = verify_offsets(&self.fs, &self.path, list, "scan", "num", false)?;
            if let Some(b) = bad.first() {
                rep.push(
                    Finding::error(
                        "index_mismatch",
                        format!(
                            "{} of {} index entries do not point at their <scan> (first: num {})",
                            bad.len(),
                            list.len(),
                            b.id
                        ),
                    )
                    .at(b.offset),
                );
            }
            let w = walk_mzxml(&self.fs, &self.path, false)?;
            let found = w.located("scan").len();
            if found != list.len() {
                rep.push(Finding::error(
                    "index_mismatch",
                    format!(
                        "the index lists {} scans, the file holds {found}",
                        list.len()
                    ),
                ));
            }
        }
        if let Some(c) = self
            .run
            .attr("scanCount")
            .and_then(|c| c.trim().parse::<u64>().ok())
        {
            rep.performed("compared the scan count with <msRun scanCount>");
            if c != self.scans.len() as u64 {
                rep.push(Finding::error(
                    "count_mismatch",
                    format!(
                        "<msRun scanCount=\"{c}\"> but {} scans were found",
                        self.scans.len()
                    ),
                ));
            }
        }
        if !deep {
            return Ok(rep);
        }
        rep.performed("decoded every <peaks> element (base64, zlib, pair count vs peaksCount)");
        let mut bad = 0usize;
        for i in 0..self.scans.len() {
            if let Err(e) = self.spectrum(i) {
                bad += 1;
                if bad == 1 {
                    rep.push(Finding::error("bad_scan", e.to_string()).at(self.scans[i].offset));
                }
            }
        }
        if bad > 1 {
            rep.push(Finding::error(
                "bad_scan",
                format!("{bad} scans failed to decode in total"),
            ));
        }
        Ok(rep)
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let file_len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        let mut notes = Vec::new();
        let mut written_index = None;
        let mut problem = None;
        match trailer_offset(fs, path, file_len, "indexOffset")? {
            Some(at) if at > 0 && at < file_len => {
                match read_written_index(fs, path, at, "id", FMT) {
                    Ok(ix) => written_index = Some(ix),
                    Err(e) => problem = Some(format!("the index could not be read ({e})")),
                }
            }
            Some(0) | None => {}
            Some(at) => problem = Some(format!("indexOffset {at} is past the end of the file")),
        }
        let mut by_index = false;
        let mut scans = Vec::new();
        if let Some(ix) = &written_index {
            let s = ix.list("scan").unwrap_or_default().to_vec();
            let bad = verify_offsets(fs, path, &s, "scan", "num", true)?;
            if bad.is_empty() && !s.is_empty() {
                scans = s;
                by_index = true;
            } else if !bad.is_empty() {
                problem = Some(format!(
                    "{} sampled index entries do not point at their <scan> (first: num {} at byte {})",
                    bad.len(),
                    bad[0].id,
                    bad[0].offset
                ));
            }
        }
        if let Some(p) = &problem {
            notes.push(format!("{p}; scans were located by scanning the file"));
        }
        let w = walk_mzxml(fs, path, by_index)?;
        if w.root_attrs.is_empty() {
            if let Some((at, e)) = &w.error {
                return Err(Error::corrupt_at(
                    FMT,
                    *at,
                    format!("not well-formed XML: {e}"),
                ));
            }
            return Err(Error::corrupt(FMT, "no <mzXML> element"));
        }
        let complete = if by_index {
            true
        } else {
            scans = w.located("scan");
            if !w.complete {
                notes.push(format!(
                    "truncated or malformed: {}; {} complete scans are readable",
                    w.error.as_ref().map_or_else(
                        || "the file ends before </mzXML>".to_string(),
                        |(at, e)| format!("XML error at byte {at}: {e}")
                    ),
                    scans.len()
                ));
            }
            w.complete
        };
        let version = w
            .root_attrs
            .iter()
            .find(|(k, _)| k == "xmlns")
            .and_then(|(_, ns)| ns.rsplit("mzXML_").next().map(str::to_string))
            .filter(|v| !v.trim().is_empty());
        Ok(MzxmlDataset {
            path: path.to_path_buf(),
            file_len,
            version,
            root_attrs: w.root_attrs,
            header: w.header,
            run: w.run,
            scans,
            by_index,
            written_index,
            index_problem: problem,
            complete,
            notes,
            summaries: OnceLock::new(),
            fs: fs.clone(),
        })
    }

    /// `dataProcessing centroided="1"` applies to every scan that does not say otherwise.
    fn default_centroided(&self) -> bool {
        self.header
            .iter()
            .filter(|n| n.tag == "dataProcessing")
            .any(|n| n.attr("centroided") == Some("1"))
    }

    fn scan_node(&self, file: Option<&mut SourceFile>, i: usize, meta_only: bool) -> Result<Node> {
        let loc = self.scans.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum index {i} out of range (file has {} scans)",
                self.scans.len()
            ))
        })?;
        // A nested <scan> (MS2 inside MS1 in 2.x files) ends the parent's own content.
        let stop: &[&str] = if meta_only {
            &["scan", "peaks"]
        } else {
            &["scan"]
        };
        let node = match file {
            Some(f) => element_in(f, loc.offset, "scan", stop, meta_only, FMT)?,
            None => element_at(
                &self.fs, &self.path, loc.offset, "scan", stop, meta_only, FMT,
            )?,
        };
        if !meta_only && node.child("peaks").is_none() && node.incomplete {
            return Err(Error::corrupt_at(
                FMT,
                loc.offset,
                format!("scan {} is cut off by the end of the file", loc.id),
            ));
        }
        Ok(node)
    }

    fn interpret(&self, n: &Node, index: u64) -> Spectrum {
        let num = n.attr("num").and_then(|v| v.trim().parse().ok());
        let mut sp = Spectrum {
            index,
            scan_number: num.unwrap_or(index + 1),
            native_id: n.attr("num").map(|v| format!("scan={v}")),
            ms_level: n
                .attr("msLevel")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0),
            polarity: match n.attr("polarity") {
                Some("+") => "positive",
                Some("-") => "negative",
                _ => "unknown",
            }
            .into(),
            centroided: n
                .attr("centroided")
                .map_or_else(|| self.default_centroided(), |v| v.trim() == "1"),
            scan_filter: n.attr("filterLine").map(str::to_string),
            total_ion_current: attr_f64(n, "totIonCurrent"),
            base_peak_mz: attr_f64(n, "basePeakMz"),
            base_peak_intensity: attr_f64(n, "basePeakIntensity"),
            collision_energy: attr_f64(n, "collisionEnergy"),
            ..Spectrum::default()
        };
        match n.attr("retentionTime").and_then(parse_duration) {
            Some(t) => sp.rt_s = Some(t),
            None => {
                sp.extra
                    .insert("retention_time_missing".into(), json!(true));
            }
        }
        if let (Some(lo), Some(hi)) = (attr_f64(n, "startMz"), attr_f64(n, "endMz")) {
            sp.scan_window_mz = Some([lo, hi]);
        }
        if let (Some(lo), Some(hi)) = (attr_f64(n, "lowMz"), attr_f64(n, "highMz")) {
            sp.extra.insert("observed_mz_range".into(), json!([lo, hi]));
        }
        for k in [
            "scanType",
            "msInstrumentID",
            "ionisationEnergy",
            "cidGasPressure",
        ] {
            if let Some(v) = n.attr(k) {
                sp.extra.insert(
                    match k {
                        "scanType" => "scan_type",
                        "msInstrumentID" => "instrument_id",
                        "ionisationEnergy" => "ionisation_energy",
                        _ => "cid_gas_pressure",
                    }
                    .into(),
                    json!(v),
                );
            }
        }
        if let Some(p) = n.children_named("precursorMz").last() {
            sp.precursor_mz = p.text.trim().parse::<f64>().ok().filter(|v| v.is_finite());
            sp.precursor_charge = attr_f64(p, "precursorCharge").map(|c| c as i32);
            sp.precursor_intensity = attr_f64(p, "precursorIntensity");
            sp.activation = p
                .attr("activationMethod")
                .map(|a| a.trim().to_ascii_uppercase())
                .filter(|a| !a.is_empty());
            if let (Some(m), Some(w)) = (sp.precursor_mz, attr_f64(p, "windowWideness")) {
                sp.isolation_window_mz = Some([m - w / 2.0, m + w / 2.0]);
            }
            if let Some(s) = p.attr("precursorScanNum") {
                sp.extra.insert("precursor_scan".into(), json!(s));
            }
        }
        sp
    }

    fn peaks(
        &self,
        n: &Node,
        offset: u64,
    ) -> std::result::Result<(Vec<f64>, Vec<f64>), ArrayError> {
        let Some(p) = n.child("peaks") else {
            return Ok((Vec::new(), Vec::new()));
        };
        let value_type = match p.attr("precision").map(str::trim) {
            Some("64") => ValueType::Float64,
            Some("32") | None => ValueType::Float32,
            Some(other) => return Err(ArrayError::Unsupported(format!("peaks precision {other}"))),
        };
        let compression = match p.attr("compressionType").map(str::trim) {
            Some("zlib") => Compression::Zlib,
            Some("none") | None => Compression::NoCompression,
            Some(other) => {
                return Err(ArrayError::Unsupported(format!(
                    "peaks compression {other}"
                )));
            }
        };
        let layout = p
            .attr("contentType")
            .or_else(|| p.attr("pairOrder"))
            .unwrap_or("m/z-int");
        if layout != "m/z-int" {
            return Err(ArrayError::Unsupported(format!(
                "peaks content type {layout}"
            )));
        }
        let big_endian = !matches!(p.attr("byteOrder"), Some("little" | "little-endian"));
        let want = n
            .attr("peaksCount")
            .and_then(|v| v.trim().parse::<usize>().ok());
        let (vals, _) = decode_array(
            p.text.as_bytes(),
            &ArrayEncoding {
                value_type,
                compression,
                big_endian,
            },
            want.map(|w| w * 2),
        )?;
        let _ = offset;
        if vals.len() % 2 != 0 {
            return Err(ArrayError::Layout(
                "odd number of values in m/z-intensity pairs".into(),
            ));
        }
        if let Some(w) = want
            && vals.len() / 2 != w
        {
            return Err(ArrayError::Layout(format!(
                "peaks holds {} pairs, peaksCount says {w}",
                vals.len() / 2
            )));
        }
        let (mut mz, mut it) = (
            Vec::with_capacity(vals.len() / 2),
            Vec::with_capacity(vals.len() / 2),
        );
        for &[m, i] in vals.as_chunks::<2>().0 {
            mz.push(m);
            it.push(i);
        }
        Ok((mz, it))
    }

    /// Visit the header of every scan from index `first` on (the `<scan>` element's attributes
    /// and precursor, up to its peaks; nothing decoded), with one open handle. `visit` returns
    /// false to stop.
    pub fn visit_headers(
        &self,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<()> {
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let start = usize::try_from(first).unwrap_or(usize::MAX);
        for i in start..self.scans.len() {
            let n = self.scan_node(Some(&mut f), i, true)?;
            let mut h = openreadout_core::ScanHeader::from(self.interpret(&n, i as u64));
            h.point_count = n.attr("peaksCount").and_then(|v| v.trim().parse().ok());
            if !visit(h) {
                break;
            }
        }
        Ok(())
    }

    pub fn spectrum(&self, i: usize) -> Result<Spectrum> {
        let n = self.scan_node(None, i, false)?;
        let offset = self.scans[i].offset;
        let mut sp = self.interpret(&n, i as u64);
        let (mz, it) = self.peaks(&n, offset).map_err(|e| match e {
            ArrayError::Unsupported(w) => Error::unsupported(
                FMT,
                w,
                "Convert the file to mzML (e.g. msconvert) to read it.",
            ),
            other => Error::corrupt_at(FMT, offset, format!("scan {}: {other}", self.scans[i].id)),
        })?;
        sp.mz = mz;
        sp.intensity = it.iter().map(|&v| v as f32).collect();
        Ok(sp)
    }

    fn summaries(&self) -> Result<&[Summary]> {
        let r = self.summaries.get_or_init(|| {
            let mut f = self.fs.open(&self.path).map_err(|e| e.to_string())?;
            let mut out = Vec::with_capacity(self.scans.len());
            for i in 0..self.scans.len() {
                let n = self
                    .scan_node(Some(&mut f), i, true)
                    .map_err(|e| e.to_string())?;
                let sp = self.interpret(&n, i as u64);
                out.push(Summary {
                    ms_level: sp.ms_level,
                    rt_s: n.attr("retentionTime").and_then(parse_duration),
                    polarity: sp.polarity,
                    centroided: sp.centroided,
                    points: n
                        .attr("peaksCount")
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0),
                });
            }
            Ok(out)
        });
        r.as_deref().map_err(|e| Error::corrupt(FMT, e.clone()))
    }

    fn instrument(&self) -> Option<InstrumentInfo> {
        let mi = self.header.iter().find(|n| n.tag == "msInstrument")?;
        let val = |tag: &str| {
            mi.child(tag)
                .and_then(|c| c.attr("value"))
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        };
        let sw = mi.child("software");
        Some(InstrumentInfo {
            manufacturer: val("msManufacturer"),
            model: val("msModel"),
            software: sw
                .and_then(|s| s.attr("name"))
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            software_version: sw
                .and_then(|s| s.attr("version"))
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            detector: val("msDetector"),
        })
    }

    fn spectra_info(&self) -> Result<SpectraInfo> {
        let sums = self.summaries()?;
        let mut levels = BTreeSet::new();
        let mut counts: BTreeMap<u32, u64> = BTreeMap::new();
        let mut pol = BTreeSet::new();
        let (mut cen, mut prof, mut pts) = (0u64, 0u64, 0u64);
        let mut rt: Option<[f64; 2]> = None;
        for s in sums {
            levels.insert(s.ms_level);
            *counts.entry(s.ms_level).or_default() += 1;
            pol.insert(s.polarity.clone());
            if s.centroided {
                cen += 1;
            } else {
                prof += 1;
            }
            pts += s.points;
            if let Some(t) = s.rt_s {
                rt = Some(rt.map_or([t, t], |[a, b]| [a.min(t), b.max(t)]));
            }
        }
        let mut extra = BTreeMap::new();
        if let Some(c) = self.run.attr("scanCount") {
            extra.insert(
                "declared_scan_count".into(),
                json!(c.trim().parse::<u64>().ok()),
            );
        }
        for (k, a) in [("start_time_s", "startTime"), ("end_time_s", "endTime")] {
            if let Some(t) = self.run.attr(a).and_then(parse_duration) {
                extra.insert(k.into(), json!(t));
            }
        }
        extra.insert("ms_level_counts".into(), json!(counts));
        extra.insert("polarities".into(), json!(pol));
        extra.insert("centroid_spectra".into(), json!(cen));
        extra.insert("profile_spectra".into(), json!(prof));
        extra.insert("total_points".into(), json!(pts));
        extra.insert("indexed".into(), json!(self.written_index.is_some()));
        extra.insert(
            "located_by".into(),
            json!(if self.by_index { "index" } else { "scan" }),
        );
        let parents: Vec<Value> = self
            .header
            .iter()
            .filter(|n| n.tag == "parentFile")
            .map(|n| json!({"name": n.attr("fileName"), "type": n.attr("fileType"), "sha1": n.attr("fileSha1")}))
            .collect();
        if !parents.is_empty() {
            extra.insert("source_files".into(), json!(parents));
        }
        if let Some(mi) = self.header.iter().find(|n| n.tag == "msInstrument") {
            let val = |tag: &str| {
                mi.child(tag)
                    .and_then(|c| c.attr("value"))
                    .map(str::to_string)
            };
            extra.insert("ionisation".into(), json!(val("msIonisation")));
            extra.insert("mass_analyzer".into(), json!(val("msMassAnalyzer")));
        }
        let dps: Vec<Value> = self
            .header
            .iter()
            .filter(|n| n.tag == "dataProcessing")
            .map(|n| {
                json!({
                    "centroided": n.attr("centroided"),
                    "software": n.children_named("software").map(|s| json!({"type": s.attr("type"), "name": s.attr("name"), "version": s.attr("version")})).collect::<Vec<_>>(),
                })
            })
            .collect();
        extra.insert("data_processing".into(), json!(dps));
        Ok(SpectraInfo {
            index: 0,
            name: self
                .header
                .iter()
                .find(|n| n.tag == "parentFile")
                .and_then(|n| n.attr("fileName"))
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string()),
            scan_count: self.scans.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s: rt,
            instrument: self.instrument(),
            extra,
        })
    }
}

impl Dataset for MzxmlDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: crate::MzxmlReader.descriptor(),
            format_version: self.version.clone(),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()?],
            traces: Vec::new(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut root = serde_json::Map::new();
        for (k, v) in &self.root_attrs {
            root.insert(format!("@{k}"), json!(v));
        }
        let mut run = match self.run.to_json() {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        for n in &self.header {
            let v = n.to_json();
            match run.get_mut(&n.tag) {
                Some(Value::Array(a)) => a.push(v),
                Some(existing) => {
                    let prev = existing.take();
                    *existing = Value::Array(vec![prev, v]);
                }
                None => {
                    run.insert(n.tag.clone(), v);
                }
            }
        }
        root.insert("msRun".into(), Value::Object(run));
        Ok(json!({"mzXML": root}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].ms_levels",
            "spectra[0].rt_range_s",
            "spectra[0].instrument.model",
            "spectra[0].extra.ms_level_counts",
            "spectra[0].extra.data_processing",
        ] {
            m.insert(k.into(), Source::Spec);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for n in &self.header {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: n.tag.clone(),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        for (i, s) in self.scans.iter().enumerate().take(LS_SCANS) {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("scan {}", s.id),
                offset: Some(s.offset),
                size: None,
                image: None,
                details: json!({"index": i}),
            });
        }
        if self.scans.len() > LS_SCANS {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("… {} more scans", self.scans.len() - LS_SCANS),
                offset: None,
                size: None,
                image: None,
                details: json!({"listed": LS_SCANS, "total": self.scans.len()}),
            });
        }
        if let Some(ix) = &self.written_index {
            out.push(LsEntry {
                kind: "index".into(),
                name: "index".into(),
                offset: Some(ix.at),
                size: Some(self.file_len.saturating_sub(ix.at)),
                image: None,
                details: json!({"entries": ix.list("scan").map_or(0, <[Located]>::len)}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FMT,
            "image planes",
            "mzXML holds mass spectra, not images: use `openreadout scans` or `export --format mzml`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.run_check(true)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        self.run_check(false)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        if run != 0 {
            return Ok(None);
        }
        Ok(Some(self.summaries()?.iter().map(|s| s.ms_level).collect()))
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (mzXML files hold one run)"
            )));
        }
        self.visit_headers(first, visit)?;
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} out of range (mzXML files hold one run)"
            )));
        }
        let i = usize::try_from(spectrum)
            .map_err(|_| Error::Usage("spectrum index too large".into()))?;
        self.spectrum(i)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 {
            return Ok(None);
        }
        let want = scan_number.to_string();
        self.scans
            .iter()
            .position(|s| s.id.trim() == want)
            .map(|i| Some(i as u64))
            .ok_or_else(|| Error::Usage(format!("no scan with num=\"{scan_number}\"")))
    }
}

pub(crate) fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: FMT.into(),
        name: "mzXML (ISB)".into(),
        vendor: "ISB/SPC open format (converted from any vendor)".into(),
        extensions: vec!["mzxml".into(), "mzxml.gz".into()],
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: false,
        confidence: crate::assurance::MZXML.confidence,
        known_gaps: vec![
            "`peaks` content types other than m/z-int pairs (e.g. m/z ruler) are not decoded (exit 6)".into(),
            "No chromatograms: mzXML stores none".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT353.43S"), Some(353.43));
        assert_eq!(parse_duration("PT5M3S"), Some(303.0));
        assert_eq!(parse_duration("P1DT1H"), Some(90_000.0));
        assert_eq!(parse_duration("PT0S"), Some(0.0));
        assert_eq!(parse_duration("353"), None);
        assert_eq!(parse_duration("PT1X"), None);
    }
}
