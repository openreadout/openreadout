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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const CREATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    let run = info.spectra.get(opts.run as usize).ok_or_else(|| {
        Error::Usage(format!(
            "run {} does not exist ({} spectra runs in this file)",
            opts.run,
            info.spectra.len()
        ))
    })?;
    let n = run.scan_count;
    let (first, last) = match opts.index_range {
        Some((a, b)) if a <= b && b < n => (a, b),
        Some((a, b)) => {
            return Err(Error::Usage(format!(
                "spectrum range {a}..={b} is outside 0..{n}"
            )));
        }
        None if n == 0 => {
            return Err(Error::Usage("the run has no spectra".into()));
        }
        None => (0, n - 1),
    };
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
        let head = header_xml(&info, run, input, thermo_ids, vendor_ids, last - first + 1);
        w.write_str(&head)?;
        let mut offsets = Vec::new();
        let mut hashes = Vec::new();
        let mut points = 0u64;
        for (k, i) in (first..=last).enumerate() {
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
            offsets.push((id.clone(), w.pos + 6)); // after the six-space indent
            points += sp.mz.len() as u64;
            let (xml, hash) = spectrum_xml(&sp, k as u64, &id)?;
            hashes.push(hash);
            w.write_str(&xml)?;
        }
        w.write_str("    </spectrumList>\n  </run>\n</mzML>\n")?;
        let index_offset = w.pos + 2;
        let mut idx = String::from("  <indexList count=\"1\">\n    <index name=\"spectrum\">\n");
        for (id, off) in &offsets {
            let _ = writeln!(idx, "      <offset idRef=\"{}\">{off}</offset>", escape(id));
        }
        idx.push_str("    </index>\n  </indexList>\n");
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

fn header_xml(
    info: &openreadout_core::FileInfo,
    run: &openreadout_core::SpectraInfo,
    input: &Path,
    thermo: bool,
    vendor_ids: bool,
    count: u64,
) -> String {
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
    if thermo {
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
    if !model.is_empty() {
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
    let _ = writeln!(
        s,
        "    <software id=\"acquisition\" version=\"{}\">\n      {}\n    </software>",
        escape(inst.software_version.as_deref().unwrap_or("unknown")),
        cv(
            "MS:1000531",
            "software",
            inst.software.as_deref().unwrap_or("acquisition software")
        )
    );
    s.push_str("  </softwareList>\n");
    let ans = analyzers(run);
    let _ = writeln!(s, "  <instrumentConfigurationList count=\"{}\">", ans.len());
    for (k, a) in ans.iter().enumerate() {
        let (an, det) = analyzer_cv(a);
        let _ = writeln!(
            s,
            "    <instrumentConfiguration id=\"IC{}\">\n      <referenceableParamGroupRef ref=\"CommonInstrumentParams\"/>\n      <componentList count=\"3\">\n        <source order=\"1\">\n          {}\n        </source>\n        <analyzer order=\"2\">\n          {an}\n        </analyzer>\n        <detector order=\"3\">\n          {det}\n        </detector>\n      </componentList>\n      <softwareRef ref=\"acquisition\"/>\n    </instrumentConfiguration>",
            k + 1,
            source_cv(run)
        );
    }
    s.push_str("  </instrumentConfigurationList>\n");
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
    let _ = writeln!(
        s,
        "  <run id=\"{}\" defaultInstrumentConfigurationRef=\"IC1\"{start} defaultSourceFileRef=\"RAW1\">",
        escape(&id)
    );
    let _ = writeln!(
        s,
        "    <spectrumList count=\"{count}\" defaultDataProcessingRef=\"openreadout_conversion\">"
    );
    let _ = info;
    s
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

fn spectrum_xml(sp: &Spectrum, index: u64, id: &str) -> Result<(String, u128)> {
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
        Some("ITMS") => " instrumentConfigurationRef=\"IC2\"",
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
    let mut vals = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        vals.push(v);
    }
    if vals.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3);
    for q in vals.chunks(4) {
        let pad = usize::from(q[2] == 64) + usize::from(q[3] == 64);
        let n = q
            .iter()
            .fold(0u32, |acc, &v| (acc << 6) | u32::from(v & 63));
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad.min(2)]);
    }
    Some(out)
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
    let mut sha = Sha1::new();
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut chunk = vec![0u8; 1 << 20];
    let mut left = covered;
    while left > 0 {
        let n = (left.min(chunk.len() as u64)) as usize;
        file.read_exact(&mut chunk[..n])
            .map_err(|e| e.to_string())?;
        sha.update(&chunk[..n]);
        left -= n as u64;
    }
    if sha.finish_hex() != w.sha {
        return Err("SHA-1 checksum does not match the content".into());
    }
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
    if !at_offset(w.index_offset, b"<indexList")? {
        return Err("indexListOffset does not point at <indexList>".into());
    }
    // Well-formed XML and every array decodes to the bytes we hashed.
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut reader = Reader::from_reader(BufReader::with_capacity(1 << 20, file));
    let mut buf = Vec::new();
    let mut in_binary = false;
    let mut arrays: Vec<Vec<u8>> = Vec::new();
    let mut k = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == "binary" => in_binary = true,
            Ok(Event::End(e)) if e.name().as_ref() == "binary" => in_binary = false,
            Ok(Event::Empty(e)) if e.name().as_ref() == "binary" => arrays.push(Vec::new()),
            Ok(Event::Text(t)) if in_binary => {
                let txt: &str = t.as_ref();
                let comp = base64_decode(txt).ok_or("bad base64")?;
                let mut out = Vec::new();
                ZlibDecoder::new(comp.as_slice())
                    .read_to_end(&mut out)
                    .map_err(|e| format!("zlib: {e}"))?;
                arrays.push(out);
            }
            Ok(Event::End(e)) if e.name().as_ref() == "spectrum" => {
                if arrays.len() != 2 {
                    return Err(format!("spectrum {k}: {} arrays", arrays.len()));
                }
                let mut h = arrays[0].clone();
                h.extend_from_slice(&arrays[1]);
                if Some(&xxhash_rust::xxh3::xxh3_128(&h)) != w.hashes.get(k) {
                    return Err(format!("spectrum {k}: arrays differ after writing"));
                }
                arrays.clear();
                k += 1;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML error at {}: {e}", reader.buffer_position())),
            _ => {}
        }
        buf.clear();
    }
    if k != w.hashes.len() {
        return Err(format!("{k} spectra read back, {} written", w.hashes.len()));
    }
    Ok(())
}

/// SHA-1 (FIPS 180-4), used only for the mzML `fileChecksum` element.
#[derive(Clone)]
struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Sha1 {
    fn new() -> Self {
        Sha1 {
            h: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            buf: Vec::with_capacity(64),
            len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block: [u8; 64] = self.buf[..].try_into().expect("64 bytes");
                self.block(&block);
                self.buf.clear();
            }
        }
        let (blocks, rest) = data.as_chunks::<64>();
        for block in blocks {
            self.block(block);
        }
        self.buf.extend_from_slice(rest);
    }

    #[allow(clippy::many_single_char_names)] // the standard's variable names
    fn block(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut bb, mut c, mut d, mut e] = self.h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((bb & c) | (!bb & d), 0x5A82_7999),
                20..=39 => (bb ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((bb & c) | (bb & d) | (c & d), 0x8F1B_BCDC),
                _ => (bb ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = bb.rotate_left(30);
            bb = a;
            a = t;
        }
        for (h, v) in self.h.iter_mut().zip([a, bb, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }

    fn finish_hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        let rem = (self.len + 1) % 64;
        let zeros = if rem <= 56 { 56 - rem } else { 120 - rem };
        pad.extend(std::iter::repeat_n(0u8, zeros as usize));
        pad.extend_from_slice(&bits.to_be_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        self.h.iter().fold(String::with_capacity(40), |mut s, x| {
            let _ = write!(s, "{x:08x}");
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
