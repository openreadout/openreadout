//! Containers around an mzML/mzXML document: gzip (`.mzML.gz`, `.mzXML.gz`: how PRIDE and
//! MetaboLights often serve them) and mzMLb (HDF5, see [`crate::mzmlb`]).
//!
//! A gzip file is decompressed once at open into a random-access view
//! ([`openreadout_core::gzip::GzipSource`]: restart points every megabyte or so, no temporary
//! file), and the ordinary mzML/mzXML reader runs on that view. [`ContainerDataset`] forwards
//! every call to that reader and adds what only the container knows: the real path, size and
//! format, the container facts under `extra`, a note, and the problems `check` must report.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::Result;
use openreadout_core::gzip::{self, GzipSource};
use openreadout_core::model::{
    AttachmentInfo, CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, Spectrum, Table,
    Trace,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, PlaneIndex, SpectrumView};
use openreadout_core::source::{Fs, Input};
use serde_json::{Value, json};

/// Decompressed bytes looked at when sniffing a gzip head.
const PEEK: usize = 16 * 1024;

/// The first decompressed bytes of a gzip head, when `head` is gzip.
pub(crate) fn peek(head: &[u8]) -> Option<Vec<u8>> {
    if gzip::is_gzip(head) {
        gzip::peek(head, PEEK)
    } else {
        None
    }
}

/// Does `path` end in `.<ext>.gz` (any case)?
pub(crate) fn has_gz_extension(path: &Path, ext: &str) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.strip_suffix(".gz")
        .is_some_and(|s| s.ends_with(&format!(".{ext}")))
}

/// True when the input's first bytes are a gzip signature.
pub(crate) fn input_is_gzip(input: &Input) -> bool {
    let Ok(f) = input.open() else {
        return false;
    };
    let mut head = [0u8; 3];
    f.read_exact_at(0, &mut head).is_ok() && gzip::is_gzip(&head)
}

/// Open a gzip-compressed input with `open` (the plain reader, given the decompressed view).
pub(crate) fn open_gz(
    input: &Input,
    format: &'static str,
    open: impl FnOnce(&Fs, &Path) -> Result<Box<dyn Dataset>>,
) -> Result<Box<dyn Dataset>> {
    let src = input
        .fs()
        .source(input.path())
        .map_err(|e| openreadout_core::Error::io(input.path(), e))?;
    let file_name = input
        .path()
        .file_name()
        .map_or_else(|| "data".to_string(), |n| n.to_string_lossy().into_owned());
    let inner_name = gzip::strip_gz(&file_name).to_string();
    let gz = GzipSource::open(src, inner_name.clone(), format)?;
    let s = gz.summary().clone();
    let view = Input::from_source(&inner_name, Arc::new(gz));
    let inner = open(view.fs(), view.path())?;
    let mut facts = json!({
        "container": "gzip",
        "members": s.members,
        "compressed_bytes": s.compressed_len,
        "decompressed_bytes": s.decompressed_len,
        "restart_points": s.checkpoints,
    });
    if let Some(n) = &s.header.name {
        facts["original_name"] = json!(n);
    }
    if let Some(p) = &s.problem {
        facts["problem"] = json!(p);
    }
    let mut findings = Vec::new();
    if let Some(p) = &s.problem {
        findings.push(if p.contains("not gzip data") {
            Finding::warning("gzip_trailing_data", p.clone())
        } else if p.contains("truncated") {
            Finding::error("gzip_truncated", p.clone())
        } else {
            Finding::error("gzip_corrupt", p.clone())
        });
    }
    let mut notes = vec![format!(
        "gzip-compressed: {} bytes decompress to {}; the file was decompressed once at open (byte offsets in this output refer to the decompressed document)",
        s.compressed_len, s.decompressed_len
    )];
    if let Some(p) = &s.problem {
        notes.push(format!("gzip: {p}"));
    }
    Ok(Box::new(ContainerDataset {
        inner,
        path: input.path().to_path_buf(),
        size_bytes: s.compressed_len,
        format: None,
        key: "compression",
        facts,
        notes,
        performed: vec![format!(
            "decompressed the gzip container ({} member(s)) and verified each member's CRC-32 and length",
            s.members
        )],
        findings,
    }))
}

/// An mzML/mzXML dataset read through a view of its container.
pub struct ContainerDataset {
    pub(crate) inner: Box<dyn Dataset>,
    pub(crate) path: PathBuf,
    pub(crate) size_bytes: u64,
    /// Replaces the inner reader's format (mzMLb), when set.
    pub(crate) format: Option<FormatDescriptor>,
    /// Key under `extra` for the container facts (`compression`, `container`).
    pub(crate) key: &'static str,
    pub(crate) facts: Value,
    /// Put first among `info` notes.
    pub(crate) notes: Vec<String>,
    /// Added to `check`'s `checks_performed` and findings.
    pub(crate) performed: Vec<String>,
    pub(crate) findings: Vec<Finding>,
}

impl std::fmt::Debug for ContainerDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerDataset")
            .field("path", &self.path)
            .field("facts", &self.facts)
            .finish_non_exhaustive()
    }
}

impl ContainerDataset {
    fn with_findings(&self, mut rep: CheckReport) -> CheckReport {
        rep.path = self.path.display().to_string();
        if let Some(f) = &self.format {
            rep.format.clone_from(&f.id);
        }
        for p in &self.performed {
            rep.performed(p.clone());
        }
        for f in &self.findings {
            rep.push(f.clone());
        }
        rep
    }
}

impl Dataset for ContainerDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut info = self.inner.info()?;
        info.path = self.path.display().to_string();
        info.size_bytes = self.size_bytes;
        if let Some(f) = &self.format {
            info.format = f.clone();
        }
        for run in &mut info.spectra {
            run.extra.insert(self.key.into(), self.facts.clone());
        }
        for t in &mut info.traces {
            t.extra.insert(self.key.into(), self.facts.clone());
        }
        let mut notes = self.notes.clone();
        notes.append(&mut info.notes);
        info.notes = notes;
        Ok(info)
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut v = self.inner.vendor_metadata()?;
        if let Value::Object(m) = &mut v {
            m.insert(self.key.into(), self.facts.clone());
        }
        Ok(v)
    }

    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }

    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        self.inner.assurance_observations()
    }
    fn file_assurance(&self) -> Option<openreadout_core::assurance::Assurance> {
        self.inner.file_assurance()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.inner.read_plane(image, index)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let rep = self.inner.check()?;
        Ok(self.with_findings(rep))
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let rep = self.inner.check_headers()?;
        Ok(self.with_findings(rep))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        self.inner.read_table(index, first_row, max_rows)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        self.inner
            .read_trace(index, sweep, first_sample, max_samples)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.inner.read_spectrum(index, spectrum)
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        self.inner.visit_scan_headers(run, first, visit)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        self.inner.spectrum_ms_levels(run)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        self.inner.find_spectrum(run, scan_number)
    }

    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        self.inner.read_spectrum_view(index, spectrum, view)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        self.inner.attachments()
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        self.inner.read_attachment(index)
    }

    fn experiment(&self) -> Option<openreadout_core::experiment::Experiment> {
        self.inner.experiment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gz_extensions() {
        assert!(has_gz_extension(Path::new("/a/b/run.mzML.gz"), "mzml"));
        assert!(has_gz_extension(Path::new("RUN.MZXML.GZ"), "mzxml"));
        assert!(!has_gz_extension(Path::new("run.mzML"), "mzml"));
        assert!(!has_gz_extension(Path::new("run.mzXML.gz"), "mzml"));
    }
}
