//! mzML 1.1 (and the 1.0/0.99 drafts that share its structure) — the dataset.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, InstrumentInfo, LsEntry, SignalChannelInfo,
    SpectraInfo, Spectrum, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Plane, Result};

use crate::binary::{ArrayEncoding, ArrayError, Compression, ValueType, decode_array};
use crate::cv::{self, Param, ParamGroups, find, has, params, value_f64};
use crate::scan::{
    Located, WrittenIndex, read_written_index, trailer_offset, verify_offsets, walk,
};
use crate::xml::{Node, element_at};

pub(crate) const FMT: &str = "mzml";

/// Elements directly under `mzML` that make up the header.
const HEADER_TAGS: &[&str] = &[
    "cvList",
    "fileDescription",
    "referenceableParamGroupList",
    "sampleList",
    "softwareList",
    "scanSettingsList",
    "instrumentConfigurationList",
    "dataProcessingList",
    "acquisitionSettingsList",
    "instrumentList",
];

/// Largest number of spectra `info --view structure` lists one by one.
const LS_SPECTRA: usize = 1000;

/// How the spectra were located.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Located_ {
    /// From the file's `indexList`, spot-checked at open.
    Index,
    /// By scanning the file, with the reason.
    Scan(String),
}

/// Per-spectrum metadata gathered on demand for `info` (no arrays).
#[derive(Debug, Clone, Default)]
struct Summary {
    ms_level: u32,
    rt_s: Option<f64>,
    polarity: String,
    centroided: bool,
    points: u64,
}

/// Chromatogram metadata (for `traces`).
#[derive(Debug, Clone, Default)]
struct ChromMeta {
    id: String,
    kind: Option<Param>,
    points: u64,
    arrays: Vec<(String, Option<String>)>,
    extra: BTreeMap<String, Value>,
}

/// An open mzML file.
#[derive(Debug)]
pub struct MzmlDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file_len: u64,
    version: Option<String>,
    root_attrs: Vec<(String, String)>,
    header: Vec<Node>,
    run: Node,
    lists: Vec<Node>,
    groups: ParamGroups,
    spectra: Vec<Located>,
    chromatograms: Vec<Located>,
    located: Located_,
    written_index: Option<WrittenIndex>,
    /// Why the file's own index could not be used, when it has one.
    index_problem: Option<String>,
    complete: bool,
    notes: Vec<String>,
    summaries: OnceLock<std::result::Result<Vec<Summary>, String>>,
    chrom_meta: OnceLock<std::result::Result<Vec<ChromMeta>, String>>,
    /// imzML: the `.ibd` file holding the arrays.
    imaging: Option<Imaging>,
    /// mzMLb: the HDF5 datasets holding the arrays.
    pub(crate) hdf5: Option<std::sync::Arc<dyn crate::mzmlb::ExternalArrays>>,
}

/// Where a spectrum's arrays live when they are not base64 text inside the XML.
#[derive(Clone, Copy)]
pub(crate) enum External<'a> {
    /// In the XML (`<binary>` text).
    Inline,
    /// imzML: byte ranges of the `.ibd` file.
    Ibd(&'a Imaging),
    /// mzMLb: slices of HDF5 datasets.
    Hdf5(&'a dyn crate::mzmlb::ExternalArrays),
}

impl External<'_> {
    fn is_inline(self) -> bool {
        matches!(self, External::Inline)
    }
}

/// The external binary file of an imzML dataset.
#[derive(Debug, Clone)]
pub(crate) struct Imaging {
    ibd: PathBuf,
    ibd_len: u64,
    fs: Fs,
}

/// `<stem>.ibd` next to an imzML file (any capitalisation of the extension).
fn find_ibd(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    for ext in ["ibd", "IBD", "Ibd"] {
        let p = dir.join(format!("{stem}.{ext}"));
        if fs.is_file(&p) {
            return Some(p);
        }
    }
    None
}

fn walk_mzml(fs: &Fs, path: &Path, stop_at_data: bool) -> Result<crate::scan::Walk> {
    walk(
        fs,
        path,
        &["mzML"],
        "run",
        HEADER_TAGS,
        &["spectrumList", "chromatogramList"],
        &["spectrum", "chromatogram"],
        stop_at_data,
    )
}

impl MzmlDataset {
    /// Open an imzML file and the `.ibd` that holds its arrays.
    pub fn open_imzml(path: &Path) -> Result<Self> {
        Self::open_imzml_in(&Fs::local(), path)
    }

    /// [`MzmlDataset::open_imzml`] in the namespace `fs`.
    pub(crate) fn open_imzml_in(fs: &Fs, path: &Path) -> Result<Self> {
        let ibd = find_ibd(fs, path).ok_or_else(|| Error::Io {
            path: path.with_extension("ibd"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "imzML arrays live in a .ibd file with the same stem, which is missing",
            ),
        })?;
        let ibd_len = fs.metadata(&ibd).map_err(|e| Error::io(&ibd, e))?.len();
        let mut ds = Self::open_in(fs, path)?;
        ds.imaging = Some(Imaging {
            ibd,
            ibd_len,
            fs: fs.clone(),
        });
        Ok(ds)
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        Self::open_in_with(fs, path, None)
    }

    /// Open `path` in the namespace `fs`; `given` is an offset index kept outside the XML
    /// (mzMLb's `mzML_spectrumIndex` datasets) used instead of an `indexList` trailer.
    pub(crate) fn open_in_with(fs: &Fs, path: &Path, given: Option<WrittenIndex>) -> Result<Self> {
        let file_len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        let mut notes = Vec::new();
        // The index first: when it is good, only the header needs to be read.
        let mut written_index = None;
        let mut index_problem = None;
        if let Some(ix) = given {
            written_index = Some(ix);
        } else {
            match trailer_offset(fs, path, file_len, "indexListOffset")? {
                Some(at) if at < file_len => match read_written_index(fs, path, at, "idRef", FMT) {
                    Ok(ix) => written_index = Some(ix),
                    Err(e) => {
                        index_problem = Some(format!("the index could not be read ({e})"));
                    }
                },
                Some(at) => {
                    index_problem =
                        Some(format!("indexListOffset {at} is past the end of the file"));
                }
                None => {}
            }
        }
        let mut located = Located_::Scan("the file has no offset index".into());
        let mut spectra = Vec::new();
        let mut chromatograms = Vec::new();
        if let Some(ix) = &written_index {
            let s = ix.list("spectrum").unwrap_or_default().to_vec();
            let c = ix.list("chromatogram").unwrap_or_default().to_vec();
            let bad_s = verify_offsets(fs, path, &s, "spectrum", "id", true)?;
            let bad_c = verify_offsets(fs, path, &c, "chromatogram", "id", true)?;
            if bad_s.is_empty() && bad_c.is_empty() {
                spectra = s;
                chromatograms = c;
                located = Located_::Index;
            } else {
                index_problem = Some(format!(
                    "{} sampled index entries do not point at their element (first: {:?} at byte {})",
                    bad_s.len() + bad_c.len(),
                    bad_s
                        .first()
                        .or(bad_c.first())
                        .map_or("", |l| l.id.as_str()),
                    bad_s.first().or(bad_c.first()).map_or(0, |l| l.offset)
                ));
            }
        }
        if let Some(p) = &index_problem {
            located = Located_::Scan(p.clone());
            notes.push(format!("{p}; spectra were located by scanning the file"));
        }
        let w = walk_mzml(fs, path, located == Located_::Index)?;
        if w.root_attrs.is_empty() {
            if let Some((at, e)) = &w.error {
                return Err(Error::corrupt_at(
                    FMT,
                    *at,
                    format!("not well-formed XML: {e}"),
                ));
            }
            return Err(Error::corrupt(FMT, "no <mzML> element"));
        }
        let complete = if located == Located_::Index {
            true
        } else {
            spectra = w.located("spectrum");
            chromatograms = w.located("chromatogram");
            if !w.complete {
                let why = match &w.error {
                    Some((at, e)) => format!("XML error at byte {at}: {e}"),
                    None => "the file ends before </mzML>".into(),
                };
                notes.push(format!(
                    "truncated or malformed: {why}; {} complete spectra are readable",
                    spectra.len()
                ));
            }
            w.complete
        };
        let groups = cv::param_groups(
            w.header
                .iter()
                .find(|n| n.tag == "referenceableParamGroupList"),
        );
        let version = w
            .root_attrs
            .iter()
            .find(|(k, _)| k == "version")
            .map(|(_, v)| v.trim().to_string())
            // an empty attribute is "not recorded" (book/src/guides/metadata.md)
            .filter(|v| !v.is_empty());
        Ok(MzmlDataset {
            path: path.to_path_buf(),
            file_len,
            version,
            root_attrs: w.root_attrs,
            header: w.header,
            run: w.run,
            lists: w.lists,
            groups,
            spectra,
            chromatograms,
            located,
            written_index,
            index_problem,
            complete,
            notes,
            summaries: OnceLock::new(),
            chrom_meta: OnceLock::new(),
            imaging: None,
            hdf5: None,
            fs: fs.clone(),
        })
    }

    /// Where this file's arrays live.
    pub(crate) fn external(&self) -> External<'_> {
        if let Some(h) = &self.hdf5 {
            External::Hdf5(h.as_ref())
        } else if let Some(i) = &self.imaging {
            External::Ibd(i)
        } else {
            External::Inline
        }
    }

    /// Length of the XML document in bytes.
    pub(crate) fn file_len(&self) -> u64 {
        self.file_len
    }

    fn header_node(&self, tag: &str) -> Option<&Node> {
        self.header.iter().find(|n| n.tag == tag)
    }

    fn declared_count(&self, list: &str) -> Option<u64> {
        self.lists
            .iter()
            .find(|n| n.tag == list)
            .and_then(|n| n.attr("count"))
            .and_then(|c| c.trim().parse().ok())
    }

    /// Read spectrum `i` as an element tree (`meta_only`: stop before the arrays).
    fn spectrum_node(
        &self,
        file: Option<&mut SourceFile>,
        i: usize,
        meta_only: bool,
    ) -> Result<Node> {
        let loc = self.spectra.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum index {i} out of range (file has {} spectra)",
                self.spectra.len()
            ))
        })?;
        let stop: &[&str] = if meta_only && self.imaging.is_none() && self.hdf5.is_none() {
            &["binaryDataArrayList"]
        } else {
            &[]
        };
        let node = match file {
            Some(f) => crate::xml::element_in(f, loc.offset, "spectrum", stop, meta_only, FMT)?,
            None => element_at(
                &self.fs, &self.path, loc.offset, "spectrum", stop, meta_only, FMT,
            )?,
        };
        if node.incomplete && !meta_only {
            return Err(Error::corrupt_at(
                FMT,
                loc.offset,
                format!(
                    "spectrum {i} ({}) is cut off by the end of the file",
                    loc.id
                ),
            ));
        }
        Ok(node)
    }

    fn summaries(&self) -> Result<&[Summary]> {
        let r = self.summaries.get_or_init(|| {
            let mut f = self.fs.open(&self.path).map_err(|e| e.to_string())?;
            let mut out = Vec::with_capacity(self.spectra.len());
            for i in 0..self.spectra.len() {
                let n = self
                    .spectrum_node(Some(&mut f), i, true)
                    .map_err(|e| e.to_string())?;
                let m = interpret(&n, &self.groups, i as u64);
                out.push(Summary {
                    ms_level: m.ms_level,
                    rt_s: m.rt,
                    polarity: m.spectrum.polarity.clone(),
                    centroided: m.spectrum.centroided,
                    points: m.points,
                });
            }
            Ok(out)
        });
        r.as_deref().map_err(|e| Error::corrupt(FMT, e.clone()))
    }

    fn chrom_meta(&self) -> Result<&[ChromMeta]> {
        let r = self.chrom_meta.get_or_init(|| {
            let mut out = Vec::new();
            for (i, loc) in self.chromatograms.iter().enumerate() {
                let n = element_at(
                    &self.fs,
                    &self.path,
                    loc.offset,
                    "chromatogram",
                    &[],
                    false,
                    FMT,
                )
                .map_err(|e| e.to_string())?;
                out.push(chrom_meta_of(&n, &self.groups, i, &self.path, loc.offset)?);
            }
            Ok(out)
        });
        r.as_deref().map_err(|e| Error::corrupt(FMT, e.clone()))
    }

    /// Instrument summary from the default (or first) instrument configuration.
    fn instrument(&self) -> Option<InstrumentInfo> {
        let list = self.header_node("instrumentConfigurationList")?;
        let want = self.run.attr("defaultInstrumentConfigurationRef");
        let ic = list
            .children_named("instrumentConfiguration")
            .find(|c| want.is_none() || c.attr("id") == want)
            .or_else(|| list.children_named("instrumentConfiguration").next())?;
        let ps = params(ic, &self.groups);
        // the generic `instrument model` term with no value: the model name may sit in a user
        // param (`instrument model name`, as OpenReadout writes it)
        let user_model = || {
            ps.iter()
                .find(|p| p.accession.is_empty() && p.name == "instrument model name")
                .map(|p| p.value.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let model = ps
            .iter()
            .find(|p| p.accession != "MS:1000529" && p.accession.starts_with("MS:"))
            .map(|p| {
                if p.accession == "MS:1000031" && p.value.trim().is_empty() {
                    user_model().unwrap_or_else(|| p.name.clone())
                } else {
                    p.label()
                }
            })
            .or_else(|| {
                ps.iter()
                    .find(|p| p.accession.is_empty())
                    .map(|p| p.name.clone())
            });
        let detector = ic.child("componentList").map(|cl| {
            cl.children_named("detector")
                .flat_map(|d| params(d, &self.groups))
                .map(|p| p.label())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let (software, software_version) = ic
            .child("softwareRef")
            .and_then(|r| r.attr("ref"))
            .and_then(|r| self.software(r))
            .unwrap_or((None, None));
        Some(InstrumentInfo {
            manufacturer: None,
            model,
            software: software.filter(|s| !s.trim().is_empty()),
            // OpenMS writes `version=""` when it does not know the version.
            software_version: software_version.filter(|v| !v.trim().is_empty()),
            detector: detector.filter(|d| !d.is_empty()),
        })
    }

    fn software(&self, id: &str) -> Option<(Option<String>, Option<String>)> {
        let s = self
            .header_node("softwareList")?
            .children_named("software")
            .find(|s| s.attr("id") == Some(id))?;
        let name = params(s, &self.groups)
            .into_iter()
            .next()
            .map(|p| p.label())
            .or_else(|| {
                s.child("softwareParam")
                    .and_then(|p| p.attr("name").map(str::to_string))
            });
        let version = s
            .attr("version")
            .or_else(|| s.child("softwareParam").and_then(|p| p.attr("version")))
            .map(str::to_string);
        Some((name.or_else(|| Some(id.to_string())), version))
    }

    fn spectra_info(&self) -> Result<SpectraInfo> {
        let sums = self.summaries()?;
        let mut levels = BTreeSet::new();
        let mut level_counts: BTreeMap<u32, u64> = BTreeMap::new();
        let mut polarities = BTreeSet::new();
        let (mut centroid, mut profile) = (0u64, 0u64);
        let mut rt: Option<[f64; 2]> = None;
        let mut points = 0u64;
        for s in sums {
            levels.insert(s.ms_level);
            *level_counts.entry(s.ms_level).or_default() += 1;
            polarities.insert(s.polarity.clone());
            if s.centroided {
                centroid += 1;
            } else {
                profile += 1;
            }
            points += s.points;
            if let Some(t) = s.rt_s {
                rt = Some(match rt {
                    None => [t, t],
                    Some([a, b]) => [a.min(t), b.max(t)],
                });
            }
        }
        let mut extra = BTreeMap::new();
        let run_id = self.run.attr("id").map(str::to_string);
        extra.insert("run_id".into(), json!(run_id));
        if let Some(t) = self.run.attr("startTimeStamp") {
            extra.insert("acquired_at".into(), json!(t));
        }
        if let Some(c) = self.declared_count("spectrumList") {
            extra.insert("declared_spectrum_count".into(), json!(c));
        }
        extra.insert("ms_level_counts".into(), json!(level_counts));
        extra.insert("polarities".into(), json!(polarities));
        extra.insert("centroid_spectra".into(), json!(centroid));
        extra.insert("profile_spectra".into(), json!(profile));
        extra.insert("total_points".into(), json!(points));
        extra.insert("chromatogram_count".into(), json!(self.chromatograms.len()));
        extra.insert("indexed".into(), json!(self.written_index.is_some()));
        if let Some(img) = &self.imaging {
            extra.insert("imaging".into(), self.imaging_summary(img));
        }
        extra.insert(
            "located_by".into(),
            json!(match &self.located {
                Located_::Index => "index",
                Located_::Scan(_) => "scan",
            }),
        );
        if let Some(fc) = self
            .header_node("fileDescription")
            .and_then(|f| f.child("fileContent"))
        {
            extra.insert(
                "file_content".into(),
                json!(
                    params(fc, &self.groups)
                        .iter()
                        .map(|p| p.name.clone())
                        .collect::<Vec<_>>()
                ),
            );
        }
        if let Some(sf) = self
            .header_node("fileDescription")
            .and_then(|f| f.child("sourceFileList"))
        {
            let files: Vec<Value> = sf
                .children_named("sourceFile")
                .map(|s| {
                    let ps = params(s, &self.groups);
                    json!({
                        "name": s.attr("name"),
                        "location": s.attr("location"),
                        "terms": ps.iter().map(Param::to_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            extra.insert("source_files".into(), json!(files));
        }
        if let Some(sl) = self.header_node("softwareList") {
            let sw: Vec<Value> = sl
                .children_named("software")
                .map(|s| {
                    let (name, version) = self
                        .software(s.attr("id").unwrap_or_default())
                        .unwrap_or((None, None));
                    let accession = params(s, &self.groups)
                        .into_iter()
                        .next()
                        .map(|p| p.accession)
                        .filter(|a| !a.is_empty() && !cv::GENERIC_PARENTS.contains(&a.as_str()));
                    json!({"id": s.attr("id"), "name": name, "version": version, "accession": accession})
                })
                .collect();
            extra.insert("software".into(), json!(sw));
        }
        if let Some(icl) = self.header_node("instrumentConfigurationList") {
            let ics: Vec<Value> = icl
                .children_named("instrumentConfiguration")
                .map(|ic| {
                    let comps: Vec<Value> = ic
                        .child("componentList")
                        .map(|cl| {
                            cl.children
                                .iter()
                                .filter(|c| matches!(c.tag.as_str(), "source" | "analyzer" | "detector"))
                                .map(|c| {
                                    let ps = params(c, &self.groups);
                                    json!({
                                        "kind": c.tag,
                                        "order": c.attr("order").and_then(|o| o.parse::<u32>().ok()),
                                        "terms": ps.iter().map(cv::Param::label).collect::<Vec<_>>(),
                                        // parallel to `terms`; empty for user params and generic terms
                                        "accessions": ps.iter().map(|p| if cv::GENERIC_PARENTS.contains(&p.accession.as_str()) && !p.value.trim().is_empty() { String::new() } else { p.accession.clone() }).collect::<Vec<_>>(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    json!({
                        "id": ic.attr("id"),
                        "terms": params(ic, &self.groups).iter().map(Param::to_json).collect::<Vec<_>>(),
                        "components": comps,
                    })
                })
                .collect();
            extra.insert("instrument_configurations".into(), json!(ics));
            if let Some(serial) = icl
                .children_named("instrumentConfiguration")
                .flat_map(|ic| params(ic, &self.groups))
                .find(|p| p.accession == "MS:1000529")
            {
                extra.insert("instrument_serial".into(), json!(serial.value));
            }
        }
        if let Some(dpl) = self.header_node("dataProcessingList") {
            let dps: Vec<Value> = dpl
                .children_named("dataProcessing")
                .map(|dp| {
                    let methods: Vec<Value> = dp
                        .children_named("processingMethod")
                        .map(|m| {
                            json!({
                                "software": m.attr("softwareRef"),
                                "steps": params(m, &self.groups).iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    json!({"id": dp.attr("id"), "methods": methods})
                })
                .collect();
            extra.insert("data_processing".into(), json!(dps));
        }
        let name = run_id.or_else(|| {
            self.root_attrs
                .iter()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.clone())
        });
        Ok(SpectraInfo {
            index: 0,
            name,
            scan_count: self.spectra.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s: rt,
            instrument: self.instrument(),
            extra,
        })
    }

    fn traces(&self) -> Result<Vec<TraceInfo>> {
        Ok(self
            .chrom_meta()?
            .iter()
            .enumerate()
            .map(|(i, c)| TraceInfo {
                index: i as u32,
                name: Some(c.id.clone()),
                sample_rate_hz: 0.0,
                sample_count: c.points,
                sweep_count: 1,
                channels: c
                    .arrays
                    .iter()
                    .enumerate()
                    .map(|(k, (name, unit))| SignalChannelInfo {
                        index: k as u32,
                        name: name.clone(),
                        unit: unit.clone(),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                start_s: None,
                extra: {
                    let mut e = c.extra.clone();
                    if let Some(k) = &c.kind {
                        e.insert("chromatogram_type".into(), json!(k.name));
                        e.insert("chromatogram_type_accession".into(), json!(k.accession));
                    }
                    e.insert(
                        "irregular_sampling".into(),
                        json!("sample_rate_hz is 0: channel `time` holds each sample's time in seconds"),
                    );
                    e
                },
            })
            .collect())
    }

    /// Visit the header of every spectrum from index `first` on (the `<spectrum>` element up to
    /// its binary arrays; no array decoded), with one open handle. `visit` returns false to
    /// stop.
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
        for i in start..self.spectra.len() {
            let n = self.spectrum_node(Some(&mut f), i, true)?;
            let m = interpret(&n, &self.groups, i as u64);
            let mut h = openreadout_core::ScanHeader::from(m.spectrum);
            h.point_count = Some(m.points);
            if !visit(h) {
                break;
            }
        }
        Ok(())
    }

    /// Decode spectrum `i` completely.
    pub fn spectrum(&self, i: usize) -> Result<Spectrum> {
        let node = self.spectrum_node(None, i, false)?;
        let offset = self.spectra[i].offset;
        let mut m = interpret(&node, &self.groups, i as u64);
        let arrays = decode_arrays(&node, &self.groups, m.points, offset, self.external())?;
        let mut mz = None;
        let mut intensity = None;
        let mut others = Vec::new();
        for a in arrays {
            match a.kind.as_str() {
                "mz" if mz.is_none() => mz = Some(a.values),
                "intensity" if intensity.is_none() => intensity = Some(a.values),
                _ => others.push(a.kind),
            }
        }
        let mz = mz.unwrap_or_default();
        let intensity = intensity.unwrap_or_default();
        if mz.len() != intensity.len() && !mz.is_empty() && !intensity.is_empty() {
            return Err(Error::corrupt_at(
                FMT,
                offset,
                format!(
                    "spectrum {i}: m/z array has {} values, intensity array {}",
                    mz.len(),
                    intensity.len()
                ),
            ));
        }
        if !others.is_empty() {
            m.spectrum
                .extra
                .insert("other_arrays".into(), json!(others));
        }
        m.spectrum.mz = mz;
        m.spectrum.intensity = intensity.iter().map(|&v| v as f32).collect();
        Ok(m.spectrum)
    }

    fn chromatogram_arrays(&self, i: usize) -> Result<Vec<Vec<f64>>> {
        let loc = self.chromatograms.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "trace {i} out of range (file has {} chromatograms)",
                self.chromatograms.len()
            ))
        })?;
        let n = element_at(
            &self.fs,
            &self.path,
            loc.offset,
            "chromatogram",
            &[],
            false,
            FMT,
        )?;
        if n.incomplete {
            return Err(Error::corrupt_at(
                FMT,
                loc.offset,
                "chromatogram cut off by the end of the file",
            ));
        }
        let points = n
            .attr("defaultArrayLength")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let arrays = decode_arrays(&n, &self.groups, points, loc.offset, self.external())?;
        let meta = &self.chrom_meta()?[i];
        let mut out = Vec::new();
        for (name, _) in &meta.arrays {
            let a = arrays.iter().find(|a| &a.kind == name).ok_or_else(|| {
                Error::corrupt_at(FMT, loc.offset, format!("array {name} missing"))
            })?;
            let scale = if a.kind == "time" {
                a.time_scale.unwrap_or(60.0)
            } else {
                1.0
            };
            out.push(a.values.iter().map(|v| v * scale).collect());
        }
        Ok(out)
    }

    /// Full integrity check.
    /// `deep`: also verify every index offset, scan the whole file, verify checksums and
    /// decode every array (`check`); without it only the header, completeness, declared counts
    /// and the `.ibd` UUID are checked (`check --headers-only`).
    fn run_check(&self, deep: bool) -> Result<CheckReport> {
        let fmt = if self.imaging.is_some() {
            crate::IMZML_FORMAT_ID
        } else {
            FMT
        };
        let mut rep = CheckReport::new(self.path.display().to_string(), fmt);
        rep.performed(
            "parsed the header (cvList, fileDescription, softwareList, instrument configurations)",
        );
        if !self.complete {
            rep.push(Finding::error(
                "truncated",
                "the file ends before </mzML> (interrupted write or copy)",
            ));
        }
        // index
        if let Some(p) = &self.index_problem {
            rep.push(Finding::error("index_mismatch", p.clone()));
        }
        match (&self.written_index, &self.located) {
            (Some(_), _) if !deep => {
                rep.performed("found the indexList (offsets not verified: headers-only check)");
            }
            (Some(ix), _) => {
                rep.performed("validated every indexList offset against the element it names");
                for (name, tag) in [("spectrum", "spectrum"), ("chromatogram", "chromatogram")] {
                    let list = ix.list(name).unwrap_or_default();
                    let bad = verify_offsets(&self.fs, &self.path, list, tag, "id", false)?;
                    if let Some(b) = bad.first() {
                        rep.push(
                            Finding::error(
                                "index_mismatch",
                                format!(
                                    "{} of {} {name} index entries do not point at <{tag} id=\"…\"> (first: {:?})",
                                    bad.len(),
                                    list.len(),
                                    b.id
                                ),
                            )
                            .at(b.offset),
                        );
                    }
                }
                // the index must list every spectrum the file holds
                let w = walk_mzml(&self.fs, &self.path, false)?;
                let scanned = w.located("spectrum");
                let listed = ix.list("spectrum").unwrap_or_default();
                if scanned.len() != listed.len() {
                    rep.push(Finding::error(
                        "index_mismatch",
                        format!(
                            "the index lists {} spectra, the file holds {}",
                            listed.len(),
                            scanned.len()
                        ),
                    ));
                }
                rep.performed("scanned the whole file for <spectrum> elements");
                self.check_checksum(&mut rep)?;
            }
            (None, Located_::Scan(why)) if self.index_problem.is_none() => {
                rep.push(Finding::info("no_index", why.clone()));
            }
            (None, _) => {}
        }
        if let Some(img) = &self.imaging {
            self.check_ibd(img, &mut rep, deep)?;
        }
        // counts
        if let Some(c) = self.declared_count("spectrumList") {
            rep.performed("compared the spectrum count with <spectrumList count>");
            if c != self.spectra.len() as u64 {
                rep.push(Finding::error(
                    "count_mismatch",
                    format!(
                        "<spectrumList count=\"{c}\"> but {} spectra were found",
                        self.spectra.len()
                    ),
                ));
            }
        }
        if let Some(c) = self.declared_count("chromatogramList")
            && c != self.chromatograms.len() as u64
        {
            rep.push(Finding::error(
                "count_mismatch",
                format!(
                    "<chromatogramList count=\"{c}\"> but {} chromatograms were found",
                    self.chromatograms.len()
                ),
            ));
        }
        if !deep {
            return Ok(rep);
        }
        // every spectrum decodes
        rep.performed("decoded every binary array (base64, compression, value count vs defaultArrayLength, encodedLength)");
        let mut unsupported = BTreeSet::new();
        let mut bad = 0usize;
        let mut first_bad: Option<Finding> = None;
        let mut enc_len_bad = 0usize;
        for i in 0..self.spectra.len() {
            let node = match self.spectrum_node(None, i, false) {
                Ok(n) => n,
                Err(e) => {
                    bad += 1;
                    first_bad.get_or_insert(
                        Finding::error("bad_spectrum", e.to_string()).at(self.spectra[i].offset),
                    );
                    continue;
                }
            };
            let m = interpret(&node, &self.groups, i as u64);
            match decode_arrays_checked(&node, &self.groups, m.points, self.external()) {
                Ok(n_enc_bad) => enc_len_bad += n_enc_bad,
                Err(ArrayError::Unsupported(what)) => {
                    unsupported.insert(what);
                }
                Err(e) => {
                    bad += 1;
                    first_bad.get_or_insert(
                        Finding::error(
                            "bad_array",
                            format!("spectrum {i} ({}): {e}", self.spectra[i].id),
                        )
                        .at(self.spectra[i].offset),
                    );
                }
            }
        }
        if let Some(f) = first_bad {
            rep.push(f);
            if bad > 1 {
                rep.push(Finding::error(
                    "bad_array",
                    format!("{bad} spectra failed to decode in total"),
                ));
            }
        }
        if enc_len_bad > 0 {
            rep.push(Finding::warning(
                "encoded_length_mismatch",
                format!("{enc_len_bad} arrays have a base64 length different from their encodedLength attribute"),
            ));
        }
        for u in unsupported {
            rep.push(Finding::warning(
                "unsupported_encoding",
                format!("arrays encoded with {u} were not decoded"),
            ));
        }
        for (i, loc) in self.chromatograms.iter().enumerate() {
            if let Err(e) = self.chromatogram_arrays(i) {
                rep.push(
                    Finding::error(
                        "bad_chromatogram",
                        format!("chromatogram {} ({}): {e}", i, loc.id),
                    )
                    .at(loc.offset),
                );
            }
        }
        Ok(rep)
    }

    /// imzML: storage mode, image geometry and scan pattern from the header.
    fn imaging_summary(&self, img: &Imaging) -> Value {
        let fc: Vec<Param> = self
            .header_node("fileDescription")
            .and_then(|f| f.child("fileContent"))
            .map(|c| params(c, &self.groups))
            .unwrap_or_default();
        let mode = if has(&fc, cv::CONTINUOUS) {
            Some("continuous")
        } else if has(&fc, cv::PROCESSED) {
            Some("processed")
        } else {
            None
        };
        let ss: Vec<Param> = self
            .header_node("scanSettingsList")
            .map(|l| {
                l.children_named("scanSettings")
                    .flat_map(|s| params(s, &self.groups))
                    .collect()
            })
            .unwrap_or_default();
        let int = |a: &str| find(&ss, a).and_then(|p| p.value.trim().parse::<u64>().ok());
        let um = |a: &str| value_f64(&ss, a);
        let pattern: Vec<String> = ss
            .iter()
            .filter(|p| p.accession.starts_with("IMS:10004") && p.value.is_empty())
            .map(|p| p.name.clone())
            .collect();
        json!({
            "ibd_file": img.ibd.file_name().map(|n| n.to_string_lossy().into_owned()),
            "storage": mode,
            "uuid": find(&fc, cv::IBD_UUID).map(|p| p.value.clone()),
            "pixels_x": int(cv::PIXELS_X),
            "pixels_y": int(cv::PIXELS_Y),
            "pixel_size_um": [um(cv::PIXEL_SIZE_X), um(cv::PIXEL_SIZE_Y)],
            "dimension_um": [um(cv::DIMENSION_X), um(cv::DIMENSION_Y)],
            "scan_pattern": pattern,
        })
    }

    /// imzML: the `.ibd` UUID and SHA-1 recorded in the header.
    fn check_ibd(&self, img: &Imaging, rep: &mut CheckReport, deep: bool) -> Result<()> {
        use sha1::{Digest, Sha1};
        let fc: Vec<Param> = self
            .header_node("fileDescription")
            .and_then(|f| f.child("fileContent"))
            .map(|c| params(c, &self.groups))
            .unwrap_or_default();
        let mut f = img.fs.open(&img.ibd).map_err(|e| Error::io(&img.ibd, e))?;
        if let Some(u) = find(&fc, cv::IBD_UUID) {
            rep.performed("compared the .ibd UUID with the imzML header");
            let mut head = [0u8; 16];
            let got = if f.read_exact(&mut head).is_ok() {
                head.iter().fold(String::new(), |mut s, b| {
                    use std::fmt::Write as _;
                    let _ = write!(s, "{b:02x}");
                    s
                })
            } else {
                String::new()
            };
            let want: String = u
                .value
                .chars()
                .filter(char::is_ascii_hexdigit)
                .collect::<String>()
                .to_ascii_lowercase();
            if got != want {
                rep.push(Finding::error(
                    "uuid_mismatch",
                    format!("the imzML names .ibd UUID {want}, the .ibd starts with {got}"),
                ));
            }
        }
        if let Some(s) = find(&fc, cv::IBD_SHA1).filter(|_| deep) {
            rep.performed("verified the .ibd SHA-1 recorded in the imzML header");
            f.seek(SeekFrom::Start(0))
                .map_err(|e| Error::io(&img.ibd, e))?;
            let mut h = Sha1::new();
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf).map_err(|e| Error::io(&img.ibd, e))?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            let got = h.finalize().iter().fold(String::new(), |mut acc, b| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{b:02x}");
                acc
            });
            if got != s.value.trim().to_ascii_lowercase() {
                rep.push(Finding::error(
                    "checksum_mismatch",
                    format!("ibd SHA-1 is {}, the .ibd hashes to {got}", s.value.trim()),
                ));
            }
        }
        Ok(())
    }

    fn check_checksum(&self, rep: &mut CheckReport) -> Result<()> {
        use sha1::{Digest, Sha1};
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let tail_len = self.file_len.min(4096);
        f.seek(SeekFrom::Start(self.file_len - tail_len))
            .map_err(|e| Error::io(&self.path, e))?;
        let mut tail = Vec::new();
        (&mut f)
            .take(tail_len)
            .read_to_end(&mut tail)
            .map_err(|e| Error::io(&self.path, e))?;
        let marker = b"<fileChecksum>";
        let Some(at) = tail.windows(marker.len()).rposition(|w| w == marker) else {
            return Ok(());
        };
        let rest = &tail[at + marker.len()..];
        let end = rest.iter().position(|&c| c == b'<').unwrap_or(rest.len());
        let want = String::from_utf8_lossy(&rest[..end])
            .trim()
            .to_ascii_lowercase();
        if want.is_empty() {
            return Ok(());
        }
        // A SHA-1 digest is 40 hex digits. Writers that do not compute one leave a placeholder
        // (OpenMS writes `0`; others 40 zeros): nothing can be verified from it, and it says
        // nothing about damage, so it is a warning, not a mismatch.
        if !is_sha1_digest(&want) {
            rep.push(Finding::warning(
                "checksum_placeholder",
                format!(
                    "fileChecksum holds {:?}, not a SHA-1 digest (a placeholder the writer left); the file's integrity cannot be verified from it",
                    want.chars().take(48).collect::<String>()
                ),
            ));
            return Ok(());
        }
        let upto = self.file_len - tail_len + (at + marker.len()) as u64;
        f.seek(SeekFrom::Start(0))
            .map_err(|e| Error::io(&self.path, e))?;
        let mut h = Sha1::new();
        let mut r = BufReader::with_capacity(1 << 20, f.take(upto));
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = r.read(&mut buf).map_err(|e| Error::io(&self.path, e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        let got = h.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        rep.performed("verified the SHA-1 fileChecksum");
        if got != want {
            rep.push(Finding::error(
                "checksum_mismatch",
                format!("fileChecksum says {want}, the content hashes to {got}"),
            ));
        }
        Ok(())
    }
}

/// Is `s` (lower case) a SHA-1 digest: 40 hex digits, not all zero (an all-zero value is a
/// placeholder some writers leave instead of computing the digest)?
fn is_sha1_digest(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()) && s.bytes().any(|b| b != b'0')
}

/// Does an ISO-8601 date-time carry a zone designator (`Z` or `±hh:mm`)?
fn has_zone(t: &str) -> bool {
    let time = t.split_once('T').map_or("", |(_, b)| b);
    time.ends_with('Z') || time.contains('+') || time.contains('-')
}

/// What `interpret` extracts from a spectrum element (arrays excluded).
pub(crate) struct Meta {
    pub(crate) spectrum: Spectrum,
    pub(crate) ms_level: u32,
    pub(crate) rt: Option<f64>,
    pub(crate) points: u64,
}

/// `scan=N` in a native id (the first such match), as the oracle reads it.
pub(crate) fn scan_number_of(id: &str) -> Option<u64> {
    let mut s = id;
    while let Some(i) = s.find("scan=") {
        let digits: String = s[i + 5..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(n) = digits.parse() {
            return Some(n);
        }
        s = &s[i + 5..];
    }
    None
}

/// Interpret a spectrum element's metadata (works on a metadata-only tree too).
pub(crate) fn interpret(n: &Node, groups: &ParamGroups, index: u64) -> Meta {
    let ps = params(n, groups);
    let id = n.attr("id").unwrap_or_default().to_string();
    let mut points = n
        .attr("defaultArrayLength")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if points == 0
        && let Some(count) = arrays_of(n).find_map(|b| {
            let ps = params(b, groups);
            external(&ps)
                .map(|(_, c, _)| c)
                .or_else(|| external_hdf5(&ps).map(|(_, _, c)| c))
        })
    {
        points = count;
    }
    let mut sp = Spectrum {
        index,
        scan_number: scan_number_of(&id).unwrap_or(index + 1),
        native_id: Some(id.clone()).filter(|s| !s.is_empty()),
        ..Spectrum::default()
    };
    let mut ms_level = cv::find(&ps, cv::MS_LEVEL)
        .and_then(|p| p.value.trim().parse().ok())
        .unwrap_or(0);
    if ms_level == 0 && has(&ps, cv::MS1_SPECTRUM) {
        ms_level = 1;
    }
    sp.ms_level = ms_level;
    sp.centroided = has(&ps, cv::CENTROID);
    sp.total_ion_current = value_f64(&ps, cv::TIC);
    sp.base_peak_mz = value_f64(&ps, cv::BASE_PEAK_MZ);
    sp.base_peak_intensity = value_f64(&ps, cv::BASE_PEAK_INTENSITY);
    if let (Some(lo), Some(hi)) = (
        value_f64(&ps, cv::LOWEST_MZ),
        value_f64(&ps, cv::HIGHEST_MZ),
    ) {
        sp.extra.insert("observed_mz_range".into(), json!([lo, hi]));
    }
    let mut polarity = if has(&ps, cv::POSITIVE) {
        "positive"
    } else if has(&ps, cv::NEGATIVE) {
        "negative"
    } else {
        "unknown"
    };
    // scan
    let mut rt = None;
    if let Some(sl) = n.child("scanList") {
        let scans: Vec<&Node> = sl.children_named("scan").collect();
        if scans.len() > 1 {
            sp.extra.insert("scan_count".into(), json!(scans.len()));
        }
        if let Some(scan) = scans.first() {
            let sps = params(scan, groups);
            if let Some(t) = find(&sps, cv::SCAN_START_TIME)
                && let Some(v) = t.f64()
            {
                rt = Some(v * cv::seconds_per(t).unwrap_or(60.0));
            }
            sp.scan_filter = find(&sps, cv::FILTER_STRING).map(|p| p.value.clone());
            if let Some(v) = value_f64(&sps, cv::ION_INJECTION_TIME) {
                sp.extra.insert("ion_injection_time_ms".into(), json!(v));
            }
            sp.inverse_reduced_mobility = value_f64(&sps, cv::INVERSE_REDUCED_MOBILITY);
            if polarity == "unknown" {
                if has(&sps, cv::POSITIVE) {
                    polarity = "positive";
                } else if has(&sps, cv::NEGATIVE) {
                    polarity = "negative";
                }
            }
            let pos: Vec<f64> = [cv::POSITION_X, cv::POSITION_Y, cv::POSITION_Z]
                .iter()
                .map_while(|a| value_f64(&sps, a))
                .collect();
            if pos.len() >= 2 {
                sp.extra.insert("position".into(), json!(pos));
            }
            if let Some(r) = scan.attr("instrumentConfigurationRef") {
                sp.extra.insert("instrument_configuration".into(), json!(r));
            }
            if let Some(w) = scan
                .child("scanWindowList")
                .and_then(|l| l.children_named("scanWindow").next())
            {
                let wps = params(w, groups);
                if let (Some(lo), Some(hi)) = (
                    value_f64(&wps, cv::SCAN_WINDOW_LOWER),
                    value_f64(&wps, cv::SCAN_WINDOW_UPPER),
                ) {
                    sp.scan_window_mz = Some([lo, hi]);
                }
            }
        }
    }
    sp.polarity = polarity.into();
    // precursor: the last one (MS3 lists the chain; the last is the one fragmented here)
    if let Some(p) = n
        .child("precursorList")
        .and_then(|l| l.children_named("precursor").last())
    {
        if let Some(r) = p.attr("spectrumRef") {
            sp.extra.insert("precursor_spectrum_ref".into(), json!(r));
        }
        if let Some(iw) = p.child("isolationWindow") {
            let ips = params(iw, groups);
            if let Some(t) = value_f64(&ips, cv::ISOLATION_TARGET) {
                let lo = value_f64(&ips, cv::ISOLATION_LOWER_OFFSET).unwrap_or(0.0);
                let hi = value_f64(&ips, cv::ISOLATION_UPPER_OFFSET).unwrap_or(0.0);
                sp.isolation_window_mz = Some([t - lo, t + hi]);
                sp.extra.insert("isolation_target_mz".into(), json!(t));
            }
        }
        if let Some(si) = p
            .child("selectedIonList")
            .and_then(|l| l.children_named("selectedIon").next())
        {
            let sps = params(si, groups);
            sp.precursor_mz = value_f64(&sps, cv::SELECTED_ION_MZ);
            sp.precursor_charge = find(&sps, cv::CHARGE_STATE)
                .and_then(|p| p.value.trim().parse::<f64>().ok())
                .map(|c| c as i32);
            sp.precursor_intensity = value_f64(&sps, cv::PEAK_INTENSITY);
            if sp.inverse_reduced_mobility.is_none() {
                sp.inverse_reduced_mobility = value_f64(&sps, cv::INVERSE_REDUCED_MOBILITY);
            }
        }
        if let Some(a) = p.child("activation") {
            let aps = params(a, groups);
            sp.activation = cv::activation(&aps);
            sp.collision_energy = value_f64(&aps, cv::COLLISION_ENERGY)
                .or_else(|| value_f64(&aps, cv::NORMALIZED_COLLISION_ENERGY));
        }
    }
    sp.rt_s = rt;
    if rt.is_none() {
        sp.extra
            .insert("retention_time_missing".into(), json!(true));
    }
    Meta {
        spectrum: sp,
        ms_level,
        rt,
        points,
    }
}

/// One decoded array.
struct Array {
    kind: String,
    values: Vec<f64>,
    /// Seconds per stored unit, for time arrays.
    time_scale: Option<f64>,
}

fn encoding_of(ps: &[Param]) -> std::result::Result<ArrayEncoding, ArrayError> {
    let compression = cv::compression(ps);
    let value_type = match cv::value_type(ps) {
        Some(v) => v,
        None if matches!(
            compression,
            Compression::NumpressLinear(_)
                | Compression::NumpressPic(_)
                | Compression::NumpressSlof(_)
        ) =>
        {
            ValueType::Float64
        }
        None => return Err(ArrayError::Layout("no binary data type term".into())),
    };
    Ok(ArrayEncoding {
        value_type,
        compression,
        big_endian: false,
    })
}

fn expected_len(bda: &Node, default: u64) -> u64 {
    bda.attr("arrayLength")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// imzML external array: `(byte offset, value count, byte length)` in the `.ibd`.
fn external(ps: &[Param]) -> Option<(u64, u64, u64)> {
    let u = |acc: &str| find(ps, acc).and_then(|p| p.value.trim().parse::<u64>().ok());
    Some((
        u(cv::EXTERNAL_OFFSET)?,
        u(cv::EXTERNAL_ARRAY_LENGTH)?,
        u(cv::EXTERNAL_ENCODED_LENGTH)?,
    ))
}

/// mzMLb external array: `(HDF5 dataset, offset, value count)`.
pub(crate) fn external_hdf5(ps: &[Param]) -> Option<(String, u64, u64)> {
    let u = |acc: &str| find(ps, acc).and_then(|p| p.value.trim().parse::<u64>().ok());
    // An empty name (mzdata's writer, for an empty non-standard array with arrayLength="0")
    // names no dataset: the array is read as the (empty) inline one.
    Some((
        Some(
            find(ps, cv::EXTERNAL_HDF5_DATASET)?
                .value
                .trim()
                .to_string(),
        )
        .filter(|n| !n.is_empty())?,
        u(cv::EXTERNAL_HDF5_OFFSET)?,
        u(cv::EXTERNAL_HDF5_LENGTH)?,
    ))
}

fn read_external(img: &Imaging, at: u64, len: u64) -> std::result::Result<Vec<u8>, ArrayError> {
    if at.checked_add(len).is_none_or(|end| end > img.ibd_len) {
        return Err(ArrayError::Layout(format!(
            "external array at byte {at} (+{len}) lies past the end of {} ({} bytes)",
            img.ibd.display(),
            img.ibd_len
        )));
    }
    let n =
        usize::try_from(len).map_err(|_| ArrayError::Layout("external array too large".into()))?;
    let mut f = img
        .fs
        .open(&img.ibd)
        .map_err(|e| ArrayError::Layout(format!("{}: {e}", img.ibd.display())))?;
    let mut buf = vec![0u8; n];
    f.seek(SeekFrom::Start(at))
        .and_then(|_| f.read_exact(&mut buf))
        .map_err(|e| ArrayError::Layout(format!("{}: {e}", img.ibd.display())))?;
    Ok(buf)
}

fn decode_one(
    bda: &Node,
    groups: &ParamGroups,
    default_len: u64,
    ext: External<'_>,
) -> std::result::Result<(Array, bool), ArrayError> {
    let ps = params(bda, groups);
    let enc = encoding_of(&ps)?;
    let mut want = expected_len(bda, default_len);
    let ibd = match ext {
        External::Ibd(img) => external(&ps).map(|e| (img, e)),
        _ => None,
    };
    let h5 = match ext {
        External::Hdf5(h) => external_hdf5(&ps).map(|e| (h, e)),
        _ => None,
    };
    let (values, chars) = if let Some((h, (name, at, count))) = h5 {
        // MS-Numpress arrays are stored as bytes: their length counts bytes, not values
        if !matches!(
            enc.compression,
            Compression::NumpressLinear(_)
                | Compression::NumpressPic(_)
                | Compression::NumpressSlof(_)
        ) {
            want = count;
        }
        (h.values(&name, at, count, &enc)?, 0)
    } else if let Some((img, (at, count, len))) = ibd {
        want = count;
        let raw = read_external(img, at, len)?;
        (
            crate::binary::decode_values(raw, &enc, usize::try_from(count).ok())?,
            0,
        )
    } else {
        let text = bda.child("binary").map_or("", |b| b.text.as_str());
        decode_array(text.as_bytes(), &enc, usize::try_from(want).ok())?
    };
    if values.len() as u64 != want {
        return Err(ArrayError::Layout(format!(
            "array has {} values, defaultArrayLength/arrayLength says {want}",
            values.len()
        )));
    }
    let enc_ok = !ext.is_inline()
        || bda
            .attr("encodedLength")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .is_none_or(|n| n == chars);
    let kind = cv::array_kind(&ps);
    let time_scale = if kind == "time" {
        ps.iter()
            .find(|p| !p.unit_accession.is_empty() || !p.unit_name.is_empty())
            .and_then(cv::seconds_per)
    } else {
        None
    };
    Ok((
        Array {
            kind,
            values,
            time_scale,
        },
        enc_ok,
    ))
}

fn arrays_of(n: &Node) -> impl Iterator<Item = &Node> {
    n.child("binaryDataArrayList")
        .into_iter()
        .flat_map(|l| l.children_named("binaryDataArray"))
        .filter(|b| !external_placeholder(b))
}

/// An mzMLb array that names no dataset and holds nothing (`arrayLength="0"`, *external HDF5
/// dataset* ""): mzdata's writer emits one for an empty non-standard array. It is left out.
fn external_placeholder(b: &Node) -> bool {
    b.attr("arrayLength").map(str::trim) == Some("0")
        && b.children_named("cvParam").any(|p| {
            p.attr("accession") == Some(cv::EXTERNAL_HDF5_DATASET)
                && p.attr("value").is_none_or(|v| v.trim().is_empty())
        })
}

fn decode_arrays(
    n: &Node,
    groups: &ParamGroups,
    points: u64,
    offset: u64,
    ext: External<'_>,
) -> Result<Vec<Array>> {
    let mut out = Vec::new();
    for bda in arrays_of(n) {
        match decode_one(bda, groups, points, ext) {
            Ok((a, _)) => out.push(a),
            Err(ArrayError::Unsupported(what)) => {
                return Err(Error::unsupported(
                    FMT,
                    format!("binary array encoding {what}"),
                    "Convert the file with a tool that writes zlib or MS-Numpress arrays (e.g. msconvert --zlib).",
                ));
            }
            Err(e) => return Err(Error::corrupt_at(FMT, offset, e.to_string())),
        }
    }
    Ok(out)
}

/// For `check`: decode every array; return how many had a wrong `encodedLength`.
fn decode_arrays_checked(
    n: &Node,
    groups: &ParamGroups,
    points: u64,
    ext: External<'_>,
) -> std::result::Result<usize, ArrayError> {
    let mut enc_bad = 0;
    for bda in arrays_of(n) {
        let (_, ok) = decode_one(bda, groups, points, ext)?;
        if !ok {
            enc_bad += 1;
        }
    }
    Ok(enc_bad)
}

fn chrom_meta_of(
    n: &Node,
    groups: &ParamGroups,
    i: usize,
    path: &Path,
    offset: u64,
) -> std::result::Result<ChromMeta, String> {
    let _ = (path, offset);
    let ps = params(n, groups);
    let kind = ps
        .iter()
        .find(|p| {
            p.accession == cv::TIC_CHROMATOGRAM
                || p.accession == cv::BPC_CHROMATOGRAM
                || p.name.ends_with("chromatogram")
        })
        .cloned();
    let points = n
        .attr("defaultArrayLength")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let mut arrays = Vec::new();
    for bda in arrays_of(n) {
        let aps = params(bda, groups);
        let name = cv::array_kind(&aps);
        let unit = if name == "time" {
            Some("s".to_string())
        } else {
            aps.iter()
                .find(|p| !p.unit_name.is_empty())
                .map(|p| p.unit_name.clone())
        };
        arrays.push((name, unit));
    }
    // time first, then intensity, then the rest
    arrays.sort_by_key(|(n, _)| match n.as_str() {
        "time" => 0,
        "intensity" => 1,
        _ => 2,
    });
    let mut extra = BTreeMap::new();
    for (key, el) in [("precursor", "precursor"), ("product", "product")] {
        if let Some(p) = n.child(el)
            && let Some(iw) = p.child("isolationWindow")
            && let Some(t) = value_f64(&params(iw, groups), cv::ISOLATION_TARGET)
        {
            extra.insert(format!("{key}_mz"), json!(t));
        }
    }
    if n.incomplete && arrays.is_empty() {
        return Err(format!(
            "chromatogram {i} is cut off by the end of the file"
        ));
    }
    Ok(ChromMeta {
        id: n.attr("id").unwrap_or_default().to_string(),
        kind,
        points,
        arrays,
        extra,
    })
}

impl Dataset for MzmlDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        self.imaging
            .iter()
            .filter(|i| i.fs.is_file(&i.ibd))
            .map(|i| i.ibd.clone())
            .collect()
    }
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        crate::experiment::facts(&self.header)
    }

    fn info(&self) -> Result<FileInfo> {
        let format = if self.imaging.is_some() {
            crate::ImzmlReader.descriptor()
        } else {
            crate::MzmlReader.descriptor()
        };
        let mut notes = self.notes.clone();
        let spectra = self.spectra_info()?;
        if spectra
            .extra
            .get("acquired_at")
            .and_then(Value::as_str)
            .is_some_and(|t| !has_zone(t))
        {
            notes.push("run startTimeStamp has no time zone designator: acquired_at is local wall-clock time".into());
        }
        let traces = match self.traces() {
            Ok(t) => t,
            Err(e) => {
                notes.push(format!("chromatograms could not be read: {e}"));
                Vec::new()
            }
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format,
            format_version: self.version.clone(),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![spectra],
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = serde_json::Map::new();
        let mut root = serde_json::Map::new();
        for (k, v) in &self.root_attrs {
            root.insert(format!("@{k}"), json!(v));
        }
        for n in &self.header {
            root.insert(n.tag.clone(), n.to_json());
        }
        root.insert("run".into(), self.run.to_json());
        for l in &self.lists {
            root.insert(l.tag.clone(), l.to_json());
        }
        m.insert("mzML".into(), Value::Object(root));
        Ok(Value::Object(m))
    }

    fn provenance(&self) -> ProvenanceMap {
        // Keys are paths into `info`; per-spectrum fields (`spectra --scan N --json`) are all read
        // from PSI-MS terms (Source::Spec) and documented in docs/formats/mzml.md.
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].ms_levels",
            "spectra[0].instrument.model",
            "spectra[0].instrument.software",
            "spectra[0].extra.ms_level_counts",
            "spectra[0].extra.instrument_configurations",
            "spectra[0].extra.data_processing",
        ] {
            m.insert(k.into(), Source::Spec);
        }
        // Only claim what the file can fill: imaging runs (imzML) usually carry no scan times,
        // and chromatogram traces exist only when the file has a chromatogramList.
        let has_rt = match self.summaries.get() {
            Some(Ok(sums)) => sums.iter().any(|s| s.rt_s.is_some()),
            _ => self.imaging.is_none(),
        };
        if has_rt {
            m.insert("spectra[0].rt_range_s".into(), Source::Spec);
        }
        if !self.chromatograms.is_empty() {
            for k in ["traces[].name", "traces[].channels"] {
                m.insert(k.into(), Source::Spec);
            }
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
        out.push(LsEntry {
            kind: "run".into(),
            name: self.run.attr("id").unwrap_or("run").to_string(),
            offset: None,
            size: None,
            image: None,
            details: json!({
                "spectra": self.spectra.len(),
                "chromatograms": self.chromatograms.len(),
                "located_by": match &self.located { Located_::Index => "index".to_string(), Located_::Scan(w) => format!("scan ({w})") },
            }),
        });
        for (i, s) in self.spectra.iter().enumerate().take(LS_SPECTRA) {
            let size = self
                .spectra
                .get(i + 1)
                .map(|n| n.offset.saturating_sub(s.offset));
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: s.id.clone(),
                offset: Some(s.offset),
                size,
                image: None,
                details: json!({"index": i}),
            });
        }
        if self.spectra.len() > LS_SPECTRA {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("… {} more spectra", self.spectra.len() - LS_SPECTRA),
                offset: None,
                size: None,
                image: None,
                details: json!({"listed": LS_SPECTRA, "total": self.spectra.len()}),
            });
        }
        for (i, c) in self.chromatograms.iter().enumerate() {
            out.push(LsEntry {
                kind: "chromatogram".into(),
                name: c.id.clone(),
                offset: Some(c.offset),
                size: None,
                image: None,
                details: json!({"trace": i}),
            });
        }
        if let Some(ix) = &self.written_index {
            out.push(LsEntry {
                kind: "index".into(),
                name: "indexList".into(),
                offset: Some(ix.at),
                size: Some(self.file_len.saturating_sub(ix.at)),
                image: None,
                details: json!(
                    ix.lists
                        .iter()
                        .map(|(n, v)| json!({"name": n, "entries": v.len()}))
                        .collect::<Vec<_>>()
                ),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FMT,
            "image planes",
            "mzML holds mass spectra, not images: use `openreadout spectra` or `export --to mzml`.",
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
        // Header summaries (no binary arrays), cached with `info`'s.
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
                "run {run} out of range (mzML files hold one run)"
            )));
        }
        self.visit_headers(first, visit)?;
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} out of range (mzML files hold one run)"
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
        if let Some(i) = self
            .spectra
            .iter()
            .position(|s| scan_number_of(&s.id) == Some(scan_number))
        {
            return Ok(Some(i as u64));
        }
        // ids without `scan=`: scan number is the index + 1
        if self.spectra.iter().all(|s| scan_number_of(&s.id).is_none())
            && scan_number >= 1
            && scan_number <= self.spectra.len() as u64
        {
            return Ok(Some(scan_number - 1));
        }
        Err(Error::Usage(format!(
            "no spectrum with scan number {scan_number}"
        )))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if sweep != 0 {
            return Err(Error::Usage("chromatograms have one sweep (0)".into()));
        }
        let cols = self.chromatogram_arrays(index as usize)?;
        let start = usize::try_from(first_sample).unwrap_or(usize::MAX);
        let take = usize::try_from(max_samples).unwrap_or(usize::MAX);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: cols
                .into_iter()
                .map(|c| c.into_iter().skip(start).take(take).collect())
                .collect(),
        })
    }
}

/// Descriptor of `ImzmlReader`.
pub(crate) fn imzml_descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: "imzml".into(),
        name: "imzML (mass spectrometry imaging)".into(),
        vendor: "imzML open standard (converted from any vendor)".into(),
        extensions: vec!["imzml".into(), "ibd".into()],
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: false,
        confidence: crate::assurance::IMZML.confidence,
        known_gaps: vec![
            "Pixels are exposed as spectra with `extra.position`; no ion images are rendered"
                .into(),
            "The .ibd MD5 (IMS:1000090) is not verified; its SHA-1 (IMS:1000091) and UUID are"
                .into(),
        ],
    }
}

/// Descriptor shared by `MzmlReader`.
pub(crate) fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: FMT.into(),
        name: "mzML (HUPO-PSI)".into(),
        vendor: "HUPO-PSI open standard (converted from any vendor)".into(),
        extensions: vec!["mzml".into(), "mzml.gz".into()],
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: true,
        confidence: crate::assurance::MZML.confidence,
        known_gaps: vec![
            "Arrays other than m/z, intensity and chromatogram time/intensity (ion mobility, charge, noise, ...) are listed but not returned".into(),
            "Byte-shuffled and dictionary zstd, and the truncation+prediction encodings (MS:1003089/1003090) are recognised but not decoded (exit 6)".into(),
            "gzip-compressed files (.mzML.gz) are decompressed once at every open (restart points are kept in memory, not saved): `info` costs about two decompressions of the file; mzMLb is its own format id (mzmlb)".into(),
            "Only the first scan of a spectrum and the last precursor are normalized; the rest stay in the element tree".into(),
        ],
    }
}

#[cfg(test)]
mod checksum_tests {
    use super::is_sha1_digest;

    #[test]
    fn placeholders_are_not_digests() {
        assert!(is_sha1_digest("da39a3ee5e6b4b0d3255bfef95601890afd80709"));
        assert!(!is_sha1_digest("0"));
        assert!(!is_sha1_digest("0000000000000000000000000000000000000000"));
        assert!(!is_sha1_digest("not-a-digest"));
    }
}
