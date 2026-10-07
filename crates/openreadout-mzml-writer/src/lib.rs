//! Indexed mzML 1.1.0 writer.
//!
//! Spectra come from any [`Dataset`] that implements `read_spectrum_view`. Arrays are written
//! base64 + zlib (m/z as 64-bit, intensity as 32-bit float). The file is written under a
//! temporary name, read back (well-formed XML, index offsets, SHA-1 checksum, every array
//! decoded and hash-compared to what was written), and only then renamed into place.
//!
//! Controlled-vocabulary accessions are those of the public PSI-MS and Unit ontologies.
//!
//! mzML is the open mass-spectrometry exchange format of the HUPO Proteomics Standards
//! Initiative, read by OpenMS, ProteoWizard, pyteomics, MZmine and most analysis software.
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//!
//! use openreadout_core::{Dataset, Result};
//! use openreadout_mzml_writer::{MzmlExportOptions, default_output, export_mzml};
//!
//! /// Write the centroided spectra of run 0 of an opened file as `<stem>.mzML`.
//! fn to_mzml(dataset: &mut dyn Dataset, input: &Path) -> Result<()> {
//!     let mut options = MzmlExportOptions::default();
//!     options.centroid = true;
//!     let report = export_mzml(dataset, input, &default_output(input), &options)?;
//!     println!("{} spectra, SHA-1 {}", report.spectra_written, report.sha1);
//!     Ok(())
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use openreadout_core::{Dataset, Error, Result, Spectrum, SpectrumView};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const CREATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Spectra read and compressed together, at most.
const BATCH_SPECTRA: usize = 256;
/// A batch ends once it holds this many data points (about 100 MB decoded and compressed).
const BATCH_POINTS: usize = 4 << 20;

/// What to write.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MzmlExportOptions {
    /// Run (spectra collection) index.
    pub run: u32,
    /// Write the instrument's centroid lists instead of profiles where a scan has both.
    pub centroid: bool,
    /// Replace an existing output file.
    pub overwrite: bool,
    /// Only spectra with index in `[first, last]` (zero-based, inclusive).
    pub index_range: Option<(u64, u64)>,
}

/// Result of an mzML export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MzmlExportReport {
    /// The file that was read.
    pub input: String,
    /// The mzML file written.
    pub output: String,
    /// Always `mzml`.
    pub format: String,
    /// Spectra written.
    pub spectra_written: u64,
    /// Chromatograms written (TIC, SRM and the like).
    pub chromatograms_written: u64,
    /// Data points (m/z-intensity pairs) written, over all spectra.
    pub points_written: u64,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the file was read back and every array, the index and the checksum agreed.
    pub verified: bool,
    /// `primary` or `centroid`.
    pub view: String,
    /// SHA-1 of the file up to the checksum element, as recorded in it.
    pub sha1: String,
}

/// Export run `opts.run` of `ds` (read from `input`) to `output` as indexed mzML.
pub fn export_mzml(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &MzmlExportOptions,
) -> Result<MzmlExportReport> {
    if output == input {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let info = ds.info()?;
    // A file without spectra runs (Waters MRM) can still hold chromatograms.
    let no_run = openreadout_core::SpectraInfo::default();
    let run = match info.spectra.get(opts.run as usize) {
        Some(run) => run,
        None if info.spectra.is_empty() && opts.run == 0 && opts.index_range.is_none() => &no_run,
        None => {
            return Err(Error::Usage(format!(
                "run {} does not exist ({} spectra runs in this file)",
                opts.run,
                info.spectra.len()
            )));
        }
    };
    let n = run.scan_count;
    // `None`: the run has no spectra, and the file holds only its chromatograms.
    let range = match opts.index_range {
        Some((a, b)) if a <= b && b < n => Some((a, b)),
        Some((a, b)) => {
            return Err(Error::Usage(format!(
                "spectrum range {a}..={b} is outside 0..{n}"
            )));
        }
        None if n == 0 => None,
        None => Some((0, n - 1)),
    };
    // Without spectra the chromatograms are read first: they decide the file content and
    // whether there is anything to write.
    let mut early_chroms = if range.is_some() {
        None
    } else {
        let chroms = chromatograms(ds, &info, true)?;
        if chroms.is_empty() {
            return Err(Error::Usage(
                "the run has no spectra and no chromatograms to write as mzML; `openreadout info` lists its traces and tables, which `export --format csv` writes"
                    .into(),
            ));
        }
        Some(chroms)
    };
    let chrom_types: Vec<(String, String)> =
        early_chroms.as_deref().map_or_else(Vec::new, |chroms| {
            let mut types: Vec<(String, String)> = Vec::new();
            for c in chroms {
                if !types.iter().any(|(a, _)| *a == c.accession) {
                    types.push((c.accession.clone(), c.name.clone()));
                }
            }
            types
        });
    let view = if opts.centroid {
        SpectrumView::Centroid
    } else {
        SpectrumView::Primary
    };
    let thermo_ids = info.format.id == "thermo-raw";
    let agilent_ids = info.format.id == "agilent-masshunter";
    // Waters (`function=F process=0 scan=S`) and Sciex TOF (`sample=1 period=1 cycle=C
    // experiment=E`) spectra carry their vendor native ids; Sciex MRM groups do not follow the
    // WIFF native-id format and are numbered by scan instead.
    let vendor_ids = vendor_native_ids(&info, run);
    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    let written = (|| -> Result<Written> {
        let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut w = CountingWriter::new(BufWriter::new(f), &tmp);
        let (head, source_configs) = header_xml(
            &info,
            run,
            input,
            thermo_ids,
            vendor_ids,
            range.map(|(first, last)| last - first + 1),
            &chrom_types,
        );
        w.write_str(&head)?;
        let mut offsets = Vec::new();
        let mut hashes = Vec::new();
        let mut points = 0u64;
        if let Some((first, last)) = range {
            // Spectra are read in batches. While the threads compress one batch, this thread
            // reads the next, then writes the compressed batch in order, so the file does not
            // depend on the thread count.
            let mut read_batch = |from: u64| -> Result<Vec<(Spectrum, String)>> {
                let mut batch = Vec::new();
                let mut batch_points = 0usize;
                let mut i = from;
                while i <= last && batch.len() < BATCH_SPECTRA && batch_points < BATCH_POINTS {
                    let sp = ds.read_spectrum_view(opts.run, i, view)?;
                    let id = if agilent_ids {
                        format!("scanId={}", sp.scan_number)
                    } else if vendor_ids {
                        sp.native_id
                            .clone()
                            .unwrap_or_else(|| native_id(&sp, false))
                    } else {
                        native_id(&sp, thermo_ids)
                    };
                    batch_points = batch_points.saturating_add(sp.mz.len());
                    batch.push((sp, id));
                    i += 1;
                }
                Ok(batch)
            };
            let mut batch = read_batch(first)?;
            let mut k = 0u64;
            while !batch.is_empty() {
                let next_from = first + k + batch.len() as u64;
                let (encoded, next) = rayon::join(
                    || {
                        batch
                            .par_iter()
                            .enumerate()
                            .map(|(j, (sp, id))| {
                                spectrum_xml(sp, k + j as u64, id, !source_configs)
                            })
                            .collect::<Vec<_>>()
                    },
                    || read_batch(next_from),
                );
                for ((sp, id), xml) in batch.iter().zip(encoded) {
                    let (xml, hash) = xml?;
                    offsets.push((id.clone(), w.pos + 6)); // after the six-space indent
                    points += sp.mz.len() as u64;
                    hashes.push(hash);
                    w.write_str(&xml)?;
                }
                k += batch.len() as u64;
                batch = next?;
            }
            w.write_str("    </spectrumList>\n")?;
        }
        // The source's chromatograms (mzML inputs: TIC, SRM traces, ...), as they were read.
        let chroms = match early_chroms.take() {
            Some(chroms) => chroms,
            None => chromatograms(ds, &info, false)?,
        };
        let mut chrom_offsets = Vec::new();
        let mut chrom_hashes = Vec::new();
        if !chroms.is_empty() {
            w.write_str(&format!(
                "    <chromatogramList count=\"{}\" defaultDataProcessingRef=\"openreadout_conversion\">\n",
                chroms.len()
            ))?;
            for (k, c) in chroms.iter().enumerate() {
                chrom_offsets.push((c.id.clone(), w.pos + 6));
                let (xml, hash) = chromatogram_xml(c, k as u64)?;
                chrom_hashes.push(hash);
                w.write_str(&xml)?;
            }
            w.write_str("    </chromatogramList>\n")?;
        }
        w.write_str("  </run>\n</mzML>\n")?;
        let index_offset = w.pos + 2;
        // An index lists at least one offset, so a file without spectra has no spectrum index.
        let mut idx = format!(
            "  <indexList count=\"{}\">\n",
            usize::from(range.is_some()) + usize::from(!chroms.is_empty())
        );
        if range.is_some() {
            idx.push_str("    <index name=\"spectrum\">\n");
            for (id, off) in &offsets {
                let _ = writeln!(idx, "      <offset idRef=\"{}\">{off}</offset>", escape(id));
            }
            idx.push_str("    </index>\n");
        }
        if !chroms.is_empty() {
            idx.push_str("    <index name=\"chromatogram\">\n");
            for (id, off) in &chrom_offsets {
                let _ = writeln!(idx, "      <offset idRef=\"{}\">{off}</offset>", escape(id));
            }
            idx.push_str("    </index>\n");
        }
        idx.push_str("  </indexList>\n");
        let _ = write!(
            idx,
            "  <indexListOffset>{index_offset}</indexListOffset>\n  <fileChecksum>"
        );
        w.write_str(&idx)?;
        let sha = w.sha1.clone().finish_hex();
        w.write_str(&format!("{sha}</fileChecksum>\n</indexedmzML>\n"))?;
        w.flush()?;
        Ok(Written {
            offsets: offsets.into_iter().map(|(_, o)| o).collect(),
            hashes,
            chrom_offsets: chrom_offsets.into_iter().map(|(_, o)| o).collect(),
            chrom_hashes,
            points,
            sha,
            index_offset,
        })
    })();
    let written = match written {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = verify(&tmp, &written) {
        if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
            let _ = std::fs::remove_file(&tmp);
        }
        return Err(Error::Other(format!(
            "read-back verification of the mzML failed ({e}); output discarded"
        )));
    }
    let bytes_written = std::fs::metadata(&tmp).map_or(0, |m| m.len());
    if output.exists() {
        std::fs::remove_file(output).map_err(|e| Error::io(output, e))?;
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(MzmlExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "mzml".into(),
        spectra_written: written.hashes.len() as u64,
        chromatograms_written: written.chrom_hashes.len() as u64,
        points_written: written.points,
        bytes_written,
        verified: true,
        view: if opts.centroid { "centroid" } else { "primary" }.into(),
        sha1: written.sha,
    })
}

/// Default output path: the input with `.mzML`.
pub fn default_output(input: &Path) -> std::path::PathBuf {
    input.with_extension("mzML")
}

struct Written {
    offsets: Vec<u64>,
    hashes: Vec<u128>,
    chrom_offsets: Vec<u64>,
    chrom_hashes: Vec<u128>,
    points: u64,
    sha: String,
    index_offset: u64,
}

struct CountingWriter<'a, W: Write> {
    inner: W,
    pos: u64,
    sha1: Sha1,
    path: &'a Path,
}

impl<'a, W: Write> CountingWriter<'a, W> {
    fn new(inner: W, path: &'a Path) -> Self {
        CountingWriter {
            inner,
            pos: 0,
            sha1: Sha1::new(),
            path,
        }
    }
    fn write_str(&mut self, s: &str) -> Result<()> {
        self.inner
            .write_all(s.as_bytes())
            .map_err(|e| Error::io(self.path, e))?;
        self.sha1.update(s.as_bytes());
        self.pos += s.len() as u64;
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        self.inner.flush().map_err(|e| Error::io(self.path, e))
    }
}

/// Whether the run's spectra carry vendor native ids the writer can declare: Waters always,
/// Sciex when the run holds no MRM groups.
fn vendor_native_ids(
    info: &openreadout_core::FileInfo,
    run: &openreadout_core::SpectraInfo,
) -> bool {
    match info.format.id.as_str() {
        "waters-raw" => true,
        "sciex-wiff" => run
            .extra
            .get("stored_spectra")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v != "SRM"),
        _ => false,
    }
}

fn native_id(sp: &Spectrum, thermo: bool) -> String {
    if thermo {
        format!(
            "controllerType=0 controllerNumber=1 scan={}",
            sp.scan_number
        )
    } else {
        format!("scan={}", sp.scan_number)
    }
}

/// XML attribute/text escaping.
pub(crate) fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn cv(acc: &str, name: &str, value: &str) -> String {
    format!(
        "<cvParam cvRef=\"{}\" accession=\"{acc}\" name=\"{name}\" value=\"{}\"/>",
        acc.split(':').next().unwrap_or("MS"),
        escape(value)
    )
}

fn cv_unit(acc: &str, name: &str, value: &str, uacc: &str, uname: &str) -> String {
    format!(
        "<cvParam cvRef=\"MS\" accession=\"{acc}\" name=\"{name}\" value=\"{}\" unitCvRef=\"{}\" unitAccession=\"{uacc}\" unitName=\"{uname}\"/>",
        escape(value),
        uacc.split(':').next().unwrap_or("MS")
    )
}

/// Instrument configurations: one per analyzer token seen in the run (`FTMS`, `ITMS`, ...).
fn analyzers(run: &openreadout_core::SpectraInfo) -> Vec<String> {
    let mut v: Vec<String> = run
        .extra
        .get("analyzers")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if v.is_empty() {
        v.push("MS".into());
    }
    v
}

fn source_cv(run: &openreadout_core::SpectraInfo) -> String {
    let src = run
        .extra
        .get("ion_sources")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str())
        .unwrap_or("");
    match src {
        "ESI" => cv("MS:1000073", "electrospray ionization", ""),
        "NSI" => cv("MS:1000398", "nanoelectrospray", ""),
        "MALDI" => cv(
            "MS:1000075",
            "matrix-assisted laser desorption ionization",
            "",
        ),
        "APCI" => cv("MS:1000070", "atmospheric pressure chemical ionization", ""),
        "EI" => cv("MS:1000389", "electron ionization", ""),
        "CI" => cv("MS:1000071", "chemical ionization", ""),
        _ => cv("MS:1000008", "ionization type", src),
    }
}

fn analyzer_cv(token: &str) -> (String, String) {
    match token {
        "FTMS" => (
            cv("MS:1000484", "orbitrap", ""),
            cv("MS:1000624", "inductive detector", ""),
        ),
        "ITMS" => (
            cv("MS:1000083", "radial ejection linear ion trap", ""),
            cv("MS:1000253", "electron multiplier", ""),
        ),
        "TQMS" | "SQMS" => (
            cv("MS:1000081", "quadrupole", ""),
            cv("MS:1000253", "electron multiplier", ""),
        ),
        "TOFMS" => (
            cv("MS:1000084", "time-of-flight", ""),
            cv("MS:1000253", "electron multiplier", ""),
        ),
        other => (
            cv("MS:1000443", "mass analyzer type", other),
            cv("MS:1000026", "detector type", ""),
        ),
    }
}

/// One instrument configuration of the source (mzML inputs): its model and serial terms
/// `(accession, name)`, and its components `(kind, order, [(accession, name)])`.
struct SourceConfig {
    terms: Vec<Term>,
    components: Vec<(String, u32, Vec<Term>)>,
}

/// A CV term: `(accession, name)`; the accession is empty when the source gave none.
type Term = (String, String);

/// The source's instrument configurations (`extra.instrument_configurations`, written by the
/// mzML reader); empty for other inputs or when a configuration lists no components.
fn source_configs(run: &openreadout_core::SpectraInfo) -> Vec<SourceConfig> {
    let Some(list) = run
        .extra
        .get("instrument_configurations")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let str_of = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut out = Vec::new();
    for c in list {
        let terms = c
            .get("terms")
            .and_then(|t| t.as_array())
            .map(|t| {
                t.iter()
                    .map(|t| (str_of(t, "accession"), str_of(t, "name")))
                    .filter(|(a, _)| !a.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut components = Vec::new();
        for (k, comp) in c
            .get("components")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .enumerate()
        {
            let kind = str_of(comp, "kind");
            if !matches!(kind.as_str(), "source" | "analyzer" | "detector") {
                continue;
            }
            let order = comp
                .get("order")
                .and_then(serde_json::Value::as_u64)
                .and_then(|o| u32::try_from(o).ok())
                .unwrap_or(k as u32 + 1);
            let names = comp.get("terms").and_then(|v| v.as_array());
            let accs = comp.get("accessions").and_then(|v| v.as_array());
            let terms: Vec<Term> = names
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(i, n)| {
                    let acc = accs
                        .and_then(|a| a.get(i))
                        .and_then(|a| a.as_str())
                        .unwrap_or_default();
                    Some((acc.to_string(), n.as_str()?.to_string()))
                })
                .collect();
            components.push((kind, order, terms));
        }
        if components.is_empty() {
            return Vec::new();
        }
        out.push(SourceConfig { terms, components });
    }
    out
}

/// A component term: its own accession when known, else the generic parent term of the
/// component kind with the name as its value.
fn component_cv(kind: &str, acc: &str, name: &str) -> String {
    if acc.starts_with("MS:") {
        return cv(&escape(acc), &escape(name), "");
    }
    match kind {
        "source" => cv("MS:1000008", "ionization type", name),
        "analyzer" => cv("MS:1000443", "mass analyzer type", name),
        _ => cv("MS:1000026", "detector type", name),
    }
}

fn header_xml(
    info: &openreadout_core::FileInfo,
    run: &openreadout_core::SpectraInfo,
    input: &Path,
    thermo: bool,
    vendor_ids: bool,
    count: Option<u64>,
    chrom_types: &[(String, String)],
) -> (String, bool) {
    let mut levels = run.ms_levels.clone();
    levels.sort_unstable();
    let name = input
        .file_name()
        .map_or_else(String::new, |s| s.to_string_lossy().to_string());
    let location = input
        .parent()
        .map(|p| format!("file://{}", p.display()))
        .unwrap_or_default();
    let id = input
        .file_stem()
        .map_or_else(|| "run".into(), |s| s.to_string_lossy().to_string());
    let inst = run.instrument.clone().unwrap_or_default();
    let serial = run
        .extra
        .get("instrument_serial")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    s.push_str("<indexedmzML xmlns=\"http://psi.hupo.org/ms/mzml\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://psi.hupo.org/ms/mzml http://psidev.info/files/ms/mzML/xsd/mzML1.1.2_idx.xsd\">\n");
    let _ = writeln!(
        s,
        "<mzML xmlns=\"http://psi.hupo.org/ms/mzml\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://psi.hupo.org/ms/mzml http://psidev.info/files/ms/mzML/xsd/mzML1.1.0.xsd\" id=\"{}\" version=\"1.1.0\">",
        escape(&id)
    );
    s.push_str("  <cvList count=\"2\">\n    <cv id=\"MS\" fullName=\"Proteomics Standards Initiative Mass Spectrometry Ontology\" version=\"4.1.0\" URI=\"https://raw.githubusercontent.com/HUPO-PSI/psi-ms-CV/master/psi-ms.obo\"/>\n    <cv id=\"UO\" fullName=\"Unit Ontology\" version=\"09:04:2014\" URI=\"https://raw.githubusercontent.com/bio-ontology-research-group/unit-ontology/master/unit.obo\"/>\n  </cvList>\n");
    s.push_str("  <fileDescription>\n    <fileContent>\n");
    if levels.contains(&1) {
        let _ = writeln!(s, "      {}", cv("MS:1000579", "MS1 spectrum", ""));
    }
    if levels.iter().any(|&l| l > 1) {
        let _ = writeln!(s, "      {}", cv("MS:1000580", "MSn spectrum", ""));
    }
    for (acc, name) in chrom_types {
        let _ = writeln!(s, "      {}", cv(&escape(acc), &escape(name), ""));
    }
    s.push_str("    </fileContent>\n    <sourceFileList count=\"1\">\n");
    let _ = writeln!(
        s,
        "      <sourceFile id=\"RAW1\" name=\"{}\" location=\"{}\">",
        escape(&name),
        escape(&location)
    );
    if thermo {
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1000768", "Thermo nativeID format", "")
        );
        let _ = writeln!(s, "        {}", cv("MS:1000563", "Thermo RAW format", ""));
    } else if info.format.id == "waters-raw" && vendor_ids {
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1000769", "Waters nativeID format", "")
        );
        let _ = writeln!(s, "        {}", cv("MS:1000526", "Waters raw format", ""));
    } else if info.format.id == "sciex-wiff" && vendor_ids {
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1000770", "WIFF nativeID format", "")
        );
        let _ = writeln!(s, "        {}", cv("MS:1000562", "ABI WIFF format", ""));
    } else if info.format.id == "agilent-masshunter" {
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1001508", "Agilent MassHunter nativeID format", "")
        );
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1001509", "Agilent MassHunter format", "")
        );
    } else {
        let _ = writeln!(
            s,
            "        {}",
            cv("MS:1000776", "scan number only nativeID format", "")
        );
    }
    s.push_str("      </sourceFile>\n    </sourceFileList>\n  </fileDescription>\n");
    s.push_str("  <referenceableParamGroupList count=\"1\">\n    <referenceableParamGroup id=\"CommonInstrumentParams\">\n");
    let model = inst.model.clone().unwrap_or_default();
    // mzML inputs: the source's own instrument configurations, with their exact terms
    let configs = source_configs(run);
    let model_term = configs
        .first()
        .and_then(|c| {
            c.terms
                .iter()
                .find(|(a, n)| a != "MS:1000529" && *n == model)
        })
        .filter(|_| !model.is_empty());
    if let Some((acc, name)) = model_term {
        let _ = writeln!(s, "      {}", cv(&escape(acc), &escape(name), ""));
    } else if thermo {
        let _ = writeln!(
            s,
            "      {}",
            cv(
                "MS:1000483",
                "Thermo Fisher Scientific instrument model",
                ""
            )
        );
    } else if info.format.id == "agilent-masshunter" {
        let _ = writeln!(
            s,
            "      {}",
            cv("MS:1000490", "Agilent instrument model", "")
        );
    } else if info.format.id == "waters-raw" {
        let _ = writeln!(
            s,
            "      {}",
            cv("MS:1000126", "Waters instrument model", "")
        );
    } else if info.format.id == "sciex-wiff" {
        let _ = writeln!(
            s,
            "      {}",
            cv("MS:1000121", "SCIEX instrument model", "")
        );
    } else {
        let _ = writeln!(s, "      {}", cv("MS:1000031", "instrument model", ""));
    }
    if !serial.is_empty() {
        let _ = writeln!(
            s,
            "      {}",
            cv("MS:1000529", "instrument serial number", serial)
        );
    }
    if !model.is_empty() && model_term.is_none() {
        let _ = writeln!(
            s,
            "      <userParam name=\"instrument model name\" value=\"{}\"/>",
            escape(&model)
        );
    }
    s.push_str("    </referenceableParamGroup>\n  </referenceableParamGroupList>\n");
    s.push_str("  <softwareList count=\"2\">\n");
    let _ = writeln!(
        s,
        "    <software id=\"openreadout\" version=\"{CREATOR_VERSION}\">\n      {}\n    </software>",
        cv(
            "MS:1000799",
            "custom unreleased software tool",
            "openreadout"
        )
    );
    // the source's own term for its software when it has one (mzML inputs)
    let software_term = inst.software.as_deref().and_then(|name| {
        run.extra
            .get("software")?
            .as_array()?
            .iter()
            .find(|e| e.get("name").and_then(|n| n.as_str()) == Some(name))?
            .get("accession")?
            .as_str()
            .map(|a| cv(&escape(a), &escape(name), ""))
    });
    let _ = writeln!(
        s,
        "    <software id=\"acquisition\" version=\"{}\">\n      {}\n    </software>",
        escape(inst.software_version.as_deref().unwrap_or("unknown")),
        software_term.unwrap_or_else(|| cv(
            "MS:1000531",
            "software",
            inst.software.as_deref().unwrap_or("acquisition software")
        ))
    );
    s.push_str("  </softwareList>\n");
    if !configs.is_empty() {
        let _ = writeln!(
            s,
            "  <instrumentConfigurationList count=\"{}\">",
            configs.len()
        );
        for (k, c) in configs.iter().enumerate() {
            let _ = writeln!(
                s,
                "    <instrumentConfiguration id=\"IC{}\">\n      <referenceableParamGroupRef ref=\"CommonInstrumentParams\"/>\n      <componentList count=\"{}\">",
                k + 1,
                c.components.len()
            );
            for (kind, order, terms) in &c.components {
                let _ = writeln!(s, "        <{kind} order=\"{order}\">");
                for (acc, name) in terms {
                    let _ = writeln!(s, "          {}", component_cv(kind, acc, name));
                }
                let _ = writeln!(s, "        </{kind}>");
            }
            s.push_str("      </componentList>\n      <softwareRef ref=\"acquisition\"/>\n    </instrumentConfiguration>\n");
        }
        s.push_str("  </instrumentConfigurationList>\n");
    }
    let ans = if configs.is_empty() {
        analyzers(run)
    } else {
        Vec::new()
    };
    if !ans.is_empty() {
        let _ = writeln!(s, "  <instrumentConfigurationList count=\"{}\">", ans.len());
    }
    for (k, a) in ans.iter().enumerate() {
        let (an, det) = analyzer_cv(a);
        let _ = writeln!(
            s,
            "    <instrumentConfiguration id=\"IC{}\">\n      <referenceableParamGroupRef ref=\"CommonInstrumentParams\"/>\n      <componentList count=\"3\">\n        <source order=\"1\">\n          {}\n        </source>\n        <analyzer order=\"2\">\n          {an}\n        </analyzer>\n        <detector order=\"3\">\n          {det}\n        </detector>\n      </componentList>\n      <softwareRef ref=\"acquisition\"/>\n    </instrumentConfiguration>",
            k + 1,
            source_cv(run)
        );
    }
    if !ans.is_empty() {
        s.push_str("  </instrumentConfigurationList>\n");
    }
    let _ = writeln!(
        s,
        "  <dataProcessingList count=\"1\">\n    <dataProcessing id=\"openreadout_conversion\">\n      <processingMethod order=\"0\" softwareRef=\"openreadout\">\n        {}\n      </processingMethod>\n    </dataProcessing>\n  </dataProcessingList>",
        cv("MS:1000544", "Conversion to mzML", "")
    );
    let start = run
        .extra
        .get("acquired_at")
        .and_then(|v| v.as_str())
        .map(|t| {
            format!(
                " startTimeStamp=\"{}\"",
                escape(t.split('.').next().unwrap_or(t))
            )
        })
        .unwrap_or_default();
    // keep the source's run id (mzML inputs) when it is a valid XML ID
    let run_id = run
        .extra
        .get("run_id")
        .and_then(|v| v.as_str())
        .filter(|r| xml_id(r) == *r)
        .map_or_else(|| xml_id(&id), str::to_string);
    let _ = writeln!(
        s,
        "  <run id=\"{}\" defaultInstrumentConfigurationRef=\"IC1\"{start} defaultSourceFileRef=\"RAW1\">",
        escape(&run_id)
    );
    // A spectrumList holds at least one spectrum: a run without spectra has none.
    if let Some(count) = count {
        let _ = writeln!(
            s,
            "    <spectrumList count=\"{count}\" defaultDataProcessingRef=\"openreadout_conversion\">"
        );
    }
    let _ = info;
    (s, !configs.is_empty())
}

/// `s` as a valid XML ID (`xs:ID`): characters other than ASCII letters, digits, `_`, `-` and
/// `.` become `_`, and an ID that does not start with a letter or `_` gets a leading `_`.
fn xml_id(s: &str) -> String {
    let mut id: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        id.insert(0, '_');
    }
    id
}

/// `(analyzer token, activation token, energy, windows)` parsed from a scan filter.
type FilterParts<'a> = (Option<&'a str>, Option<String>, Option<f64>, Vec<[f64; 2]>);

fn filter_parts(f: &str) -> FilterParts<'_> {
    let analyzer = f.split_whitespace().next();
    let mut act = None;
    let mut energy = None;
    for tok in f.split_whitespace() {
        if let Some((_, rest)) = tok.split_once('@') {
            let letters: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
            let num: String = rest[letters.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            act = Some(letters);
            energy = num.parse().ok();
        }
    }
    // `[lo-hi]`, or `[lo-hi, lo-hi, ...]` for SRM product windows.
    let windows = f
        .rfind('[')
        .map(|a| {
            let inner = &f[a + 1..f[a..].find(']').map_or(f.len(), |b| a + b)];
            inner
                .split(',')
                .filter_map(|w| {
                    let (lo, hi) = w.split_once('-')?;
                    Some([lo.trim().parse().ok()?, hi.trim().parse().ok()?])
                })
                .collect()
        })
        .unwrap_or_default();
    (analyzer, act, energy, windows)
}

fn fmt_f(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

/// `analyzer_refs`: point ion-trap (`ITMS`) scans at the second instrument configuration, as
/// written for scan-filter analyzers (not when the source's own configurations are copied).
fn spectrum_xml(
    sp: &Spectrum,
    index: u64,
    id: &str,
    analyzer_refs: bool,
) -> Result<(String, u128)> {
    let mut mz_bytes = Vec::with_capacity(sp.mz.len() * 8);
    for v in &sp.mz {
        mz_bytes.extend_from_slice(&v.to_le_bytes());
    }
    let mut in_bytes = Vec::with_capacity(sp.intensity.len() * 4);
    for v in &sp.intensity {
        in_bytes.extend_from_slice(&v.to_le_bytes());
    }
    let mut h = mz_bytes.clone();
    h.extend_from_slice(&in_bytes);
    let hash = xxhash_rust::xxh3::xxh3_128(&h);
    let mz_b64 = base64_encode(&zlib(&mz_bytes)?);
    let in_b64 = base64_encode(&zlib(&in_bytes)?);
    let (analyzer, act, energy, windows) = sp
        .scan_filter
        .as_deref()
        .map_or((None, None, None, Vec::new()), filter_parts);
    let has_token = |t: &str| {
        sp.scan_filter
            .as_deref()
            .is_some_and(|f| f.split_whitespace().any(|w| w == t))
    };
    // Readers without scan filters (mzML/mzXML inputs, timsTOF) fill the Spectrum fields instead.
    let act = act.or_else(|| sp.activation.as_deref().map(str::to_ascii_lowercase));
    let energy = energy.or(sp.collision_energy);
    let ic = match analyzer {
        Some("ITMS") if analyzer_refs => " instrumentConfigurationRef=\"IC2\"",
        _ => "",
    };
    let mut s = String::new();
    let _ = writeln!(
        s,
        "      <spectrum index=\"{index}\" id=\"{}\" defaultArrayLength=\"{}\">",
        escape(id),
        sp.mz.len()
    );
    let i = "        ";
    if has_token("SRM") {
        let _ = writeln!(s, "{i}{}", cv("MS:1000583", "SRM spectrum", ""));
    } else if has_token("CRM") {
        let _ = writeln!(s, "{i}{}", cv("MS:1000581", "CRM spectrum", ""));
    } else if sp.ms_level <= 1 {
        let _ = writeln!(s, "{i}{}", cv("MS:1000579", "MS1 spectrum", ""));
    } else {
        let _ = writeln!(s, "{i}{}", cv("MS:1000580", "MSn spectrum", ""));
    }
    let _ = writeln!(
        s,
        "{i}{}",
        cv("MS:1000511", "ms level", &sp.ms_level.to_string())
    );
    match sp.polarity.as_str() {
        "positive" => {
            let _ = writeln!(s, "{i}{}", cv("MS:1000130", "positive scan", ""));
        }
        "negative" => {
            let _ = writeln!(s, "{i}{}", cv("MS:1000129", "negative scan", ""));
        }
        _ => {}
    }
    if sp.centroided {
        let _ = writeln!(s, "{i}{}", cv("MS:1000127", "centroid spectrum", ""));
    } else {
        let _ = writeln!(s, "{i}{}", cv("MS:1000128", "profile spectrum", ""));
    }
    if let Some((k, &bi)) = sp
        .intensity
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
    {
        let _ = writeln!(
            s,
            "{i}{}",
            cv_unit(
                "MS:1000504",
                "base peak m/z",
                &fmt_f(sp.mz[k]),
                "MS:1000040",
                "m/z"
            )
        );
        let _ = writeln!(
            s,
            "{i}{}",
            cv_unit(
                "MS:1000505",
                "base peak intensity",
                &fmt_f(f64::from(bi)),
                "MS:1000131",
                "number of detector counts"
            )
        );
    }
    if let Some(t) = sp.total_ion_current {
        let _ = writeln!(s, "{i}{}", cv("MS:1000285", "total ion current", &fmt_f(t)));
    }
    if let (Some(lo), Some(hi)) = (sp.mz.first(), sp.mz.last()) {
        let _ = writeln!(
            s,
            "{i}{}",
            cv_unit(
                "MS:1000528",
                "lowest observed m/z",
                &fmt_f(*lo),
                "MS:1000040",
                "m/z"
            )
        );
        let _ = writeln!(
            s,
            "{i}{}",
            cv_unit(
                "MS:1000527",
                "highest observed m/z",
                &fmt_f(*hi),
                "MS:1000040",
                "m/z"
            )
        );
    }
    let _ = writeln!(s, "{i}<scanList count=\"1\">");
    let _ = writeln!(s, "{i}  {}", cv("MS:1000795", "no combination", ""));
    let _ = writeln!(s, "{i}  <scan{ic}>");
    // a spectrum whose source states no retention time gets none (never an invented 0)
    if let Some(rt) = sp.rt_s {
        let _ = writeln!(
            s,
            "{i}    {}",
            cv_unit(
                "MS:1000016",
                "scan start time",
                &fmt_f(rt / 60.0),
                "UO:0000031",
                "minute"
            )
        );
    }
    if let Some(f) = &sp.scan_filter {
        let _ = writeln!(s, "{i}    {}", cv("MS:1000512", "filter string", f));
    }
    if !windows.is_empty() {
        let _ = writeln!(s, "{i}    <scanWindowList count=\"{}\">", windows.len());
    }
    for [lo, hi] in &windows {
        let (lo, hi) = (*lo, *hi);
        let _ = writeln!(
            s,
            "{i}      <scanWindow>\n{i}        {}\n{i}        {}\n{i}      </scanWindow>",
            cv_unit(
                "MS:1000501",
                "scan window lower limit",
                &fmt_f(lo),
                "MS:1000040",
                "m/z"
            ),
            cv_unit(
                "MS:1000500",
                "scan window upper limit",
                &fmt_f(hi),
                "MS:1000040",
                "m/z"
            )
        );
    }
    if !windows.is_empty() {
        let _ = writeln!(s, "{i}    </scanWindowList>");
    }
    let _ = writeln!(s, "{i}  </scan>\n{i}</scanList>");
    if sp.ms_level > 1
        && let Some(p) = sp.precursor_mz
    {
        let _ = writeln!(s, "{i}<precursorList count=\"1\">\n{i}  <precursor>");
        let _ = writeln!(
            s,
            "{i}    <isolationWindow>\n{i}      {}\n{i}    </isolationWindow>",
            cv_unit(
                "MS:1000827",
                "isolation window target m/z",
                &fmt_f(p),
                "MS:1000040",
                "m/z"
            )
        );
        let _ = writeln!(
            s,
            "{i}    <selectedIonList count=\"1\">\n{i}      <selectedIon>\n{i}        {}",
            cv_unit(
                "MS:1000744",
                "selected ion m/z",
                &fmt_f(p),
                "MS:1000040",
                "m/z"
            )
        );
        if let Some(c) = sp.precursor_charge {
            let _ = writeln!(
                s,
                "{i}        {}",
                cv("MS:1000041", "charge state", &c.to_string())
            );
        }
        let _ = writeln!(s, "{i}      </selectedIon>\n{i}    </selectedIonList>");
        let _ = writeln!(s, "{i}    <activation>");
        let act_cv = match act.as_deref() {
            Some("cid") => cv("MS:1000133", "collision-induced dissociation", ""),
            Some("hcd") => cv("MS:1000422", "beam-type collision-induced dissociation", ""),
            Some("etd") => cv("MS:1000598", "electron transfer dissociation", ""),
            Some(other) => cv("MS:1000044", "dissociation method", other),
            None => cv("MS:1000044", "dissociation method", ""),
        };
        let _ = writeln!(s, "{i}      {act_cv}");
        if let Some(e) = energy {
            let _ = writeln!(
                s,
                "{i}      {}",
                cv_unit(
                    "MS:1000045",
                    "collision energy",
                    &fmt_f(e),
                    "UO:0000266",
                    "electronvolt"
                )
            );
        }
        let _ = writeln!(
            s,
            "{i}    </activation>\n{i}  </precursor>\n{i}</precursorList>"
        );
    }
    let _ = writeln!(s, "{i}<binaryDataArrayList count=\"2\">");
    let _ = writeln!(
        s,
        "{i}  <binaryDataArray encodedLength=\"{}\">\n{i}    {}\n{i}    {}\n{i}    {}\n{i}    <binary>{mz_b64}</binary>\n{i}  </binaryDataArray>",
        mz_b64.len(),
        cv("MS:1000523", "64-bit float", ""),
        cv("MS:1000574", "zlib compression", ""),
        cv_unit("MS:1000514", "m/z array", "", "MS:1000040", "m/z")
    );
    let _ = writeln!(
        s,
        "{i}  <binaryDataArray encodedLength=\"{}\">\n{i}    {}\n{i}    {}\n{i}    {}\n{i}    <binary>{in_b64}</binary>\n{i}  </binaryDataArray>",
        in_b64.len(),
        cv("MS:1000521", "32-bit float", ""),
        cv("MS:1000574", "zlib compression", ""),
        cv_unit(
            "MS:1000515",
            "intensity array",
            "",
            "MS:1000131",
            "number of detector counts"
        )
    );
    let _ = writeln!(s, "{i}</binaryDataArrayList>\n      </spectrum>");
    Ok((s, hash))
}

fn zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data)
        .and_then(|()| e.finish())
        .map_err(|err| Error::Other(format!("zlib: {err}")))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 §4).
pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Inverse of [`base64_encode`]; whitespace is ignored.
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    // 0..=63 for the alphabet, PAD for '=', SKIP for whitespace, BAD for anything else.
    const PAD: u8 = 64;
    const SKIP: u8 = 65;
    const BAD: u8 = 66;
    const TABLE: [u8; 256] = {
        let mut t = [BAD; 256];
        let mut i = 0;
        while i < 64 {
            t[B64[i] as usize] = i as u8;
            i += 1;
        }
        t[b'=' as usize] = PAD;
        t[b' ' as usize] = SKIP;
        t[b'\n' as usize] = SKIP;
        t[b'\r' as usize] = SKIP;
        t[b'\t' as usize] = SKIP;
        t
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut quad = [0u8; 4];
    let mut have = 0;
    for &c in s.as_bytes() {
        let v = TABLE[usize::from(c)];
        match v {
            SKIP => continue,
            BAD => return None,
            _ => {}
        }
        quad[have] = v;
        have += 1;
        if have == 4 {
            have = 0;
            let pad = usize::from(quad[2] == PAD) + usize::from(quad[3] == PAD);
            let n = quad
                .iter()
                .fold(0u32, |acc, &v| (acc << 6) | u32::from(v & 63));
            let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
            out.extend_from_slice(&bytes[..3 - pad.min(2)]);
        }
    }
    (have == 0).then_some(out)
}

/// Passes reads through and feeds the first `left` bytes to a SHA-1.
struct HashingReader<R> {
    inner: R,
    sha: Sha1,
    left: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let hashed = usize::try_from(self.left).map_or(n, |l| l.min(n));
        self.sha.update(&buf[..hashed]);
        self.left -= hashed as u64;
        Ok(n)
    }
}

/// One spectrum or chromatogram whose arrays are checked off the reading thread.
struct Pending {
    chromatogram: bool,
    index: usize,
    /// Base64 text of each `<binary>` element; `None` for an empty element.
    arrays: Vec<Option<Vec<u8>>>,
}

/// Check that the arrays of each item decode to the bytes whose hash was recorded, on the
/// rayon threads. Reports the first failing item in file order.
fn check_arrays(items: &[Pending], w: &Written) -> std::result::Result<(), String> {
    let results: Vec<std::result::Result<(), String>> = items
        .par_iter()
        .map(|p| {
            if !p.chromatogram && p.arrays.len() != 2 {
                return Err(format!("spectrum {}: {} arrays", p.index, p.arrays.len()));
            }
            let mut h = Vec::new();
            for a in &p.arrays {
                let Some(text) = a else { continue };
                let text = std::str::from_utf8(text).map_err(|_| "bad base64")?;
                let comp = base64_decode(text).ok_or("bad base64")?;
                ZlibDecoder::new(comp.as_slice())
                    .read_to_end(&mut h)
                    .map_err(|e| format!("zlib: {e}"))?;
            }
            let (want, what) = if p.chromatogram {
                (w.chrom_hashes.get(p.index), "chromatogram")
            } else {
                (w.hashes.get(p.index), "spectrum")
            };
            if Some(&xxhash_rust::xxh3::xxh3_128(&h)) != want {
                return Err(format!("{what} {}: arrays differ after writing", p.index));
            }
            Ok(())
        })
        .collect();
    results.into_iter().collect()
}

/// Read the written file back and check it against what we meant to write.
fn verify(path: &Path, w: &Written) -> std::result::Result<(), String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    use std::io::{BufReader, Seek, SeekFrom};
    // The file is streamed, never held in memory whole: exports of large runs are GBs.
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    // Checksum covers everything up to and including "<fileChecksum>", which sits in the
    // last few hundred bytes.
    let marker = b"<fileChecksum>";
    let tail_len = len.min(4096);
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(len - tail_len))
        .and_then(|_| (&mut file).take(tail_len).read_to_end(&mut tail))
        .map_err(|e| e.to_string())?;
    let at = tail
        .windows(marker.len())
        .rposition(|x| x == marker)
        .ok_or("no fileChecksum element")?;
    let covered = len - tail_len + (at + marker.len()) as u64;
    let mut at_offset = |o: u64, want: &[u8]| -> std::result::Result<bool, String> {
        let mut got = vec![0u8; want.len()];
        file.seek(SeekFrom::Start(o)).map_err(|e| e.to_string())?;
        Ok(file.read_exact(&mut got).is_ok() && got == want)
    };
    for &o in &w.offsets {
        if !at_offset(o, b"<spectrum")? {
            return Err(format!(
                "index offset {o} does not point at a <spectrum> element"
            ));
        }
    }
    for &o in &w.chrom_offsets {
        if !at_offset(o, b"<chromatogram")? {
            return Err(format!(
                "index offset {o} does not point at a <chromatogram> element"
            ));
        }
    }
    if !at_offset(w.index_offset, b"<indexList")? {
        return Err("indexListOffset does not point at <indexList>".into());
    }
    // One pass over the file: the SHA-1 of the covered bytes, well-formed XML, and every
    // array decodes to the bytes we hashed.
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hashing = HashingReader {
        inner: file,
        sha: Sha1::new(),
        left: covered,
    };
    let mut reader = Reader::from_reader(BufReader::with_capacity(1 << 20, &mut hashing));
    let mut buf = Vec::new();
    let mut in_binary = false;
    let mut arrays: Vec<Option<Vec<u8>>> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut pending_bytes = 0usize;
    let mut k = 0usize;
    let mut kc = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == "binary" => in_binary = true,
            Ok(Event::End(e)) if e.name().as_ref() == "binary" => in_binary = false,
            Ok(Event::Empty(e)) if e.name().as_ref() == "binary" => arrays.push(None),
            Ok(Event::Text(t)) if in_binary => {
                let txt: &str = t.as_ref();
                pending_bytes += txt.len();
                arrays.push(Some(txt.as_bytes().to_vec()));
            }
            Ok(Event::End(e))
                if e.name().as_ref() == "spectrum" || e.name().as_ref() == "chromatogram" =>
            {
                let chromatogram = e.name().as_ref() == "chromatogram";
                let index = if chromatogram { &mut kc } else { &mut k };
                pending.push(Pending {
                    chromatogram,
                    index: *index,
                    arrays: std::mem::take(&mut arrays),
                });
                *index += 1;
                if pending.len() >= BATCH_SPECTRA || pending_bytes >= BATCH_POINTS * 4 {
                    check_arrays(&pending, w)?;
                    pending.clear();
                    pending_bytes = 0;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML error at {}: {e}", reader.buffer_position())),
            _ => {}
        }
        buf.clear();
    }
    check_arrays(&pending, w)?;
    drop(reader);
    if hashing.left > 0 || hashing.sha.finish_hex() != w.sha {
        return Err("SHA-1 checksum does not match the content".into());
    }
    if k != w.hashes.len() {
        return Err(format!("{k} spectra read back, {} written", w.hashes.len()));
    }
    if kc != w.chrom_hashes.len() {
        return Err(format!(
            "{kc} chromatograms read back, {} written",
            w.chrom_hashes.len()
        ));
    }
    Ok(())
}

/// A chromatogram to write: a source trace that is an mzML chromatogram, or a column of an MRM
/// table.
struct Chromatogram {
    id: String,
    accession: String,
    name: String,
    time_s: Vec<f64>,
    intensity: Vec<f64>,
    /// Label of the counts unit (MS:1000131): the source's, which may be the older
    /// "number of counts".
    counts_unit: &'static str,
    precursor_mz: Option<f64>,
    product_mz: Option<f64>,
    /// Scan polarity term, `(accession, name)`.
    polarity: Option<(&'static str, &'static str)>,
}

/// The traces the reader marks as chromatograms (`extra.chromatogram_type_accession`, set by
/// the mzML reader), with their `time` and `intensity` channels. With `tables`, also the MRM
/// tables (Waters): their stored TIC and each `Q1 > Q3` column.
fn chromatograms(
    ds: &mut dyn Dataset,
    info: &openreadout_core::FileInfo,
    tables: bool,
) -> Result<Vec<Chromatogram>> {
    let mut out = Vec::new();
    for t in &info.traces {
        let (Some(acc), Some(name)) = (
            t.extra
                .get("chromatogram_type_accession")
                .and_then(|v| v.as_str()),
            t.extra.get("chromatogram_type").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let col = |n: &str| t.channels.iter().position(|c| c.name == n);
        let (Some(ti), Some(ii)) = (col("time"), col("intensity")) else {
            continue;
        };
        let mut tr = ds.read_trace(t.index, 0, 0, t.sample_count)?;
        let (Some(time_s), Some(intensity)) = (
            tr.channels.get_mut(ti).map(std::mem::take),
            tr.channels.get_mut(ii).map(std::mem::take),
        ) else {
            continue;
        };
        let f = |k: &str| t.extra.get(k).and_then(serde_json::Value::as_f64);
        let counts_unit = if t.channels[ii].unit.as_deref() == Some("number of counts") {
            "number of counts"
        } else {
            "number of detector counts"
        };
        out.push(Chromatogram {
            id: t
                .name
                .clone()
                .unwrap_or_else(|| format!("chromatogram={}", t.index)),
            accession: acc.to_string(),
            name: name.to_string(),
            time_s,
            intensity,
            counts_unit,
            precursor_mz: f("precursor_mz"),
            product_mz: f("product_mz"),
            polarity: None,
        });
    }
    if tables {
        table_chromatograms(ds, info, &mut out)?;
    }
    Ok(out)
}

/// Q1 and Q3 of an MRM column named `Q1 > Q3`.
fn transition_of(name: &str) -> Option<(f64, f64)> {
    let (a, b) = name.split_once('>')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Chromatograms from the tables with a retention-time column (`rt_min`): the stored TIC
/// (`tic_stored`) and one SRM chromatogram per `Q1 > Q3` column.
fn table_chromatograms(
    ds: &mut dyn Dataset,
    info: &openreadout_core::FileInfo,
    out: &mut Vec<Chromatogram>,
) -> Result<()> {
    let mut ids: std::collections::HashSet<String> = out.iter().map(|c| c.id.clone()).collect();
    for t in &info.tables {
        let Some(rt) = t.columns.iter().position(|c| c.name == "rt_min") else {
            continue;
        };
        let wanted: Vec<(usize, Option<(f64, f64)>)> = t
            .columns
            .iter()
            .enumerate()
            .filter_map(|(k, c)| {
                if c.name == "tic_stored" {
                    Some((k, None))
                } else {
                    transition_of(&c.name).map(|q| (k, Some(q)))
                }
            })
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let table = ds.read_table(t.index, 0, t.row_count)?;
        let Some(minutes) = table.columns.get(rt) else {
            continue;
        };
        let time_s: Vec<f64> = minutes.iter().map(|m| m * 60.0).collect();
        let label = t
            .name
            .clone()
            .unwrap_or_else(|| format!("table {}", t.index));
        let polarity = match t.extra.get("polarity").and_then(|v| v.as_str()) {
            Some("positive") => Some(("MS:1000130", "positive scan")),
            Some("negative") => Some(("MS:1000129", "negative scan")),
            _ => None,
        };
        for (k, q) in wanted {
            let Some(values) = table.columns.get(k) else {
                continue;
            };
            let (accession, name, base) = match q {
                None => (
                    "MS:1000235",
                    "total ion current chromatogram",
                    format!("TIC {label}"),
                ),
                Some(_) => (
                    "MS:1001473",
                    "selected reaction monitoring chromatogram",
                    format!("SRM {label} {}", t.columns[k].name),
                ),
            };
            // ids are unique within the file (a method can list a transition twice)
            let mut id = base.clone();
            let mut n = 1;
            while !ids.insert(id.clone()) {
                n += 1;
                id = format!("{base} ({n})");
            }
            out.push(Chromatogram {
                id,
                accession: accession.into(),
                name: name.into(),
                time_s: time_s.clone(),
                intensity: values.clone(),
                counts_unit: "number of detector counts",
                precursor_mz: q.map(|q| q.0),
                product_mz: q.map(|q| q.1),
                polarity,
            });
        }
    }
    Ok(())
}

/// One `<chromatogram>` element (64-bit time in seconds and intensity, zlib) and the xxh3-128
/// of its decoded arrays.
fn chromatogram_xml(c: &Chromatogram, index: u64) -> Result<(String, u128)> {
    let le = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let (tb, ib) = (le(&c.time_s), le(&c.intensity));
    let mut h = tb.clone();
    h.extend_from_slice(&ib);
    let hash = xxhash_rust::xxh3::xxh3_128(&h);
    let (t64, i64_) = (base64_encode(&zlib(&tb)?), base64_encode(&zlib(&ib)?));
    let i = "        ";
    let mut s = String::new();
    let _ = writeln!(
        s,
        "      <chromatogram index=\"{index}\" id=\"{}\" defaultArrayLength=\"{}\">",
        escape(&c.id),
        c.time_s.len()
    );
    let _ = writeln!(s, "{i}{}", cv(&escape(&c.accession), &escape(&c.name), ""));
    if let Some((acc, name)) = c.polarity {
        let _ = writeln!(s, "{i}{}", cv(acc, name, ""));
    }
    for (el, v) in [("precursor", c.precursor_mz), ("product", c.product_mz)] {
        if let Some(v) = v {
            // The schema requires a precursor's activation. An SRM precursor is fragmented by
            // collision-induced dissociation; other chromatograms do not say how.
            let activation = match el {
                "precursor" if c.accession == "MS:1001473" => format!(
                    "\n{i}  <activation>\n{i}    {}\n{i}  </activation>",
                    cv("MS:1000133", "collision-induced dissociation", "")
                ),
                "precursor" => format!("\n{i}  <activation/>"),
                _ => String::new(),
            };
            let _ = writeln!(
                s,
                "{i}<{el}>\n{i}  <isolationWindow>\n{i}    {}\n{i}  </isolationWindow>{activation}\n{i}</{el}>",
                cv_unit(
                    "MS:1000827",
                    "isolation window target m/z",
                    &fmt_f(v),
                    "MS:1000040",
                    "m/z"
                )
            );
        }
    }
    let _ = writeln!(s, "{i}<binaryDataArrayList count=\"2\">");
    let _ = writeln!(
        s,
        "{i}  <binaryDataArray encodedLength=\"{}\">\n{i}    {}\n{i}    {}\n{i}    {}\n{i}    <binary>{t64}</binary>\n{i}  </binaryDataArray>",
        t64.len(),
        cv("MS:1000523", "64-bit float", ""),
        cv("MS:1000574", "zlib compression", ""),
        cv_unit("MS:1000595", "time array", "", "UO:0000010", "second")
    );
    let _ = writeln!(
        s,
        "{i}  <binaryDataArray encodedLength=\"{}\">\n{i}    {}\n{i}    {}\n{i}    {}\n{i}    <binary>{i64_}</binary>\n{i}  </binaryDataArray>",
        i64_.len(),
        cv("MS:1000523", "64-bit float", ""),
        cv("MS:1000574", "zlib compression", ""),
        cv_unit(
            "MS:1000515",
            "intensity array",
            "",
            "MS:1000131",
            c.counts_unit
        )
    );
    let _ = writeln!(s, "{i}</binaryDataArrayList>\n      </chromatogram>");
    Ok((s, hash))
}

/// SHA-1 (FIPS 180-4), used only for the mzML `fileChecksum` element.
#[derive(Clone)]
struct Sha1(sha1::Sha1);

impl Sha1 {
    fn new() -> Self {
        Sha1(sha1::Digest::new())
    }

    fn update(&mut self, data: &[u8]) {
        sha1::Digest::update(&mut self.0, data);
    }

    fn finish_hex(self) -> String {
        sha1::Digest::finalize(self.0)
            .iter()
            .fold(String::with_capacity(40), |mut s, x| {
                let _ = write!(s, "{x:02x}");
                s
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vectors() {
        let mut s = Sha1::new();
        s.update(b"abc");
        assert_eq!(s.finish_hex(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let mut s = Sha1::new();
        s.update(b"");
        assert_eq!(s.finish_hex(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let mut s = Sha1::new();
        let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        s.update(&msg[..10]);
        s.update(&msg[10..]);
        assert_eq!(s.finish_hex(), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }

    #[test]
    fn base64_roundtrip() {
        for n in 0..20 {
            let data: Vec<u8> = (0..n).map(|x| (x * 37 + 11) as u8).collect();
            let e = base64_encode(&data);
            assert_eq!(base64_decode(&e).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
    }

    #[test]
    fn filter_parsing() {
        let (a, act, e, w) = filter_parts("FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]");
        assert_eq!(a, Some("FTMS"));
        assert_eq!(act.as_deref(), Some("cid"));
        assert_eq!(e, Some(20.0));
        assert_eq!(w, vec![[50.0, 95.0]]);
        let (_, act, _, w) = filter_parts("- c ESI SRM ms2 258.026 [78.942-78.944, 96.939-96.941]");
        assert_eq!(act, None);
        assert_eq!(w, vec![[78.942, 78.944], [96.939, 96.941]]);
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
    }
}
