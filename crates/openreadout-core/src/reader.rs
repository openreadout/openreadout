//! The traits every format crate implements, and the registry that dispatches on them.

use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{
    AttachmentInfo, CheckReport, DetectConfidence, FileInfo, FormatDescriptor, LsEntry, Spectrum,
    Table, Trace,
};
use crate::pixel::Plane;
use crate::provenance::ProvenanceMap;
use crate::source::Input;

/// Result of sniffing a file's first bytes.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Format id of the reader that matched.
    pub format_id: &'static str,
    /// How sure the match is.
    pub confidence: DetectConfidence,
    /// Why the match is less than definite, if it is.
    pub note: Option<String>,
}

/// Which representation `read_spectrum_view` returns when a scan stores more than one
/// (Fourier-transform instruments typically keep a profile *and* the instrument's centroid list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum SpectrumView {
    /// The scan's primary data: the profile when one was recorded, otherwise the centroids.
    #[default]
    Primary,
    /// The centroid list stored by the instrument software, when the scan has one; otherwise
    /// the same as `Primary`.
    Centroid,
}

/// Which plane of an image to read. All zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PlaneIndex {
    /// Channel index.
    pub c: u32,
    /// Z index (focal plane).
    pub z: u32,
    /// Time index.
    pub t: u32,
}

/// An open file. Implementations must keep `info` cheap: read headers, never pixels.
pub trait Dataset: Send {
    /// Header-only summary.
    fn info(&self) -> Result<FileInfo>;
    /// The vendor's metadata tree as JSON, names untouched.
    fn vendor_metadata(&self) -> Result<serde_json::Value>;
    /// Provenance of the normalized fields this reader fills.
    fn provenance(&self) -> ProvenanceMap;
    /// Structural listing for `info --view structure`.
    fn entries(&self) -> Result<Vec<LsEntry>>;
    /// Read one full-resolution plane. Mosaics are stitched; pyramids are ignored.
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane>;
    /// Integrity validation for `check`.
    fn check(&mut self) -> Result<CheckReport>;
    /// Integrity validation that reads headers and structure only (`check --headers-only`, and
    /// the `index` crawl): offsets, declared sizes and counts against the file length, missing
    /// parts; no decompression, no decoding of samples and no checksums over the data. Readers
    /// whose `check` decodes data override it; the default is `check`, which for most formats
    /// already reads only headers and structure.
    fn check_headers(&mut self) -> Result<CheckReport> {
        self.check()
    }
    /// Other files on disk that belong to this dataset (multi-file OME-TIFF members, CZI file
    /// parts, OIR continuation files, a SpikeGLX `.meta`/`.bin` pair, an imzML `.ibd`, VSI
    /// `.ets` stacks, ...), excluding the path that was opened. Only files that exist are
    /// listed. `index` records them as members of this dataset instead of separate records.
    /// Directory datasets (a Bruker `.d`, a ChemStation `.D`) keep the default: the directory
    /// itself is the dataset.
    fn member_files(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    /// Evidence that the file is not finished: the structures its software writes last that
    /// are absent and the planes already complete ([`crate::live::WriteState`]). `None` for
    /// finished files and for formats that do not know their write order (the default).
    /// [`crate::live::assess`] turns it into `acquisition: in_progress | interrupted`.
    fn write_state(&self) -> Option<crate::live::WriteState> {
        None
    }
    /// Rows `[first_row, first_row + max_rows)` of table `index`. Formats without tables keep the default.
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let _ = (index, first_row, max_rows);
        Err(Error::unsupported(
            "core",
            "tabular data",
            "This format exposes images, not tables.",
        ))
    }
    /// Samples `[first_sample, first_sample + max_samples)` of sweep `sweep` of trace `index`.
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let _ = (index, sweep, first_sample, max_samples);
        Err(Error::unsupported(
            "core",
            "sampled signals",
            "This format does not expose traces.",
        ))
    }
    /// Spectrum `spectrum` of run `index`. Formats without spectra keep the default.
    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        let _ = (index, spectrum);
        Err(Error::unsupported(
            "core",
            "mass spectra",
            "This format exposes images, not spectra.",
        ))
    }
    /// MS level of every spectrum of run `run`, in spectrum-index order, when the reader knows
    /// them without decoding peaks (a scan index, per-scan events, header summaries). `None`
    /// (the default) means unknown: [`spectrum_by_level`] then reads the spectra one by one.
    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        let _ = run;
        Ok(None)
    }
    /// Zero-based index of the spectrum with instrument scan number `scan_number` in run `run`.
    /// `Ok(None)` means the reader keeps no scan-number map; callers then assume scan numbers
    /// are contiguous from the first spectrum's (see [`spectrum_by_scan`]).
    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        let _ = (run, scan_number);
        Ok(None)
    }
    /// Visit the headers of run `run`'s spectra from spectrum `first` on, in index order,
    /// without decoding any peaks (scan number, MS level, RT, polarity, precursor, isolation,
    /// activation, filter, the file's stored TIC/base peak: [`crate::scans::ScanHeader`]).
    /// `visit` returns false to stop. `Ok(false)` (the default) means the reader has no
    /// header-only path; [`crate::scans::visit_scans`] then decodes the spectra.
    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(crate::scans::ScanHeader) -> bool,
    ) -> Result<bool> {
        let _ = (run, first, visit);
        Ok(false)
    }
    /// Like `read_spectrum`, choosing between stored representations. Formats that store only one
    /// keep the default, which ignores `view`.
    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        let _ = view;
        self.read_spectrum(index, spectrum)
    }
    /// Read one plane of pyramid level `level` (0 = full resolution, larger = more downsampled;
    /// `ImageInfo::pyramid_levels` says how many exist). Formats without pyramids keep the
    /// default, which serves level 0 and rejects the rest.
    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        if level == 0 {
            return self.read_plane(image, index);
        }
        Err(Error::unsupported(
            "core",
            "pyramid level reads",
            "This format does not expose downsampled pyramid levels; read level 0.",
        ))
    }
    /// Read the rectangle `region` of one plane of pyramid level `level`, in that level's pixel
    /// coordinates (see [`crate::region`]). A region outside the level is a usage error (exit
    /// 2). Readers that store pixels in tiles, subblocks or chunks override it to decode only
    /// the tiles the region touches, which also makes regions of planes too large to assemble
    /// whole ([`crate::pixel::MAX_PLANE_BYTES`]) readable. The default reads the whole plane at
    /// the level and crops it.
    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: crate::region::Region,
    ) -> Result<Plane> {
        let plane = self.read_plane_level(image, index, level)?;
        crate::region::crop(plane, region, &format!("image {image} level {level}"))
    }
    /// Whether [`Dataset::read_strips`] decodes each stored tile of `image` at `level` once for
    /// all the strips of a plane. A reader whose tiles are not on a grid (overlapping CZI
    /// mosaic tiles) returns `true`, because separate strip reads would decode the tiles that
    /// cross a strip edge once per strip. `stats` then reads the plane through
    /// [`Dataset::read_strips`] instead of as separate regions.
    fn reads_strips(&self, image: u32, level: u32) -> bool {
        let _ = (image, level);
        false
    }
    /// Read the `strips` of one plane of level `level` (regions of the level, top to bottom)
    /// and hand each to `sink` in order. The default reads each strip with
    /// [`Dataset::read_region`].
    fn read_strips(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        strips: &[crate::region::Region],
        sink: &mut dyn FnMut(Plane) -> Result<()>,
    ) -> Result<()> {
        for &r in strips {
            let p = self.read_region(image, index, level, r)?;
            sink(p)?;
        }
        Ok(())
    }
    /// Files embedded in the container (thumbnails, label images, time stamps, ...).
    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(Vec::new())
    }
    /// Raw bytes of attachment `index` (see `attachments`).
    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let _ = index;
        Err(Error::unsupported(
            "core",
            "attachments",
            "This format has no embedded attachments; `info --view structure` lists what the file contains.",
        ))
    }
    /// Per-frame acquisition records of image `image` (acquisition time, stage position,
    /// detector state, ...), too numerous for `info`. Each record is a JSON object in the
    /// reader's documented vocabulary. At most `limit` records are returned (`None` = all);
    /// the first value is the total number of records. `info --view full` stores them under
    /// `images[i].extra.frames` (see [`attach_frames`]). Formats without per-frame records keep
    /// the default (no records).
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<serde_json::Value>)> {
        let _ = (image, limit);
        Ok((0, Vec::new()))
    }
    /// Experiment facts this reader knows that the normalized model cannot carry (a Bruker NMR
    /// `title` file, an mzML `sampleList`), each value with its origin in `provenance`. They are
    /// laid over the experiment derived from `info` ([`crate::experiment::of_dataset`]). Most
    /// readers keep the default: everything they know is already in `info`.
    fn experiment(&self) -> Option<crate::experiment::Experiment> {
        None
    }
    /// The layout of a multi-well plate (high-content screening): imaged wells, the images of
    /// each, and how complete the copy on disk is. `info` reports it under `plate`. Readers
    /// of other files keep the default (none).
    fn plate(&self) -> Option<crate::plate::PlateSummary> {
        None
    }
    /// What the reader itself knows about this file's variant beyond `info` (structures met
    /// but not decoded, values assumed, calibrations, block types), laid over its assurance
    /// profile's observations ([`crate::assurance::assess`]). Most readers keep the default:
    /// their profile reads everything it needs from `info`. Wrappers forward it.
    fn assurance_observations(&self) -> crate::assurance::Observations {
        crate::assurance::Observations::default()
    }
    /// This file's assurance ([`crate::assurance::assess_dataset`] over `info`); `None` when
    /// `info` cannot be read. The strict-mode wrapper returns the assessment it gates on, so
    /// `check` reports it even when strict mode refuses `info`.
    fn file_assurance(&self) -> Option<crate::assurance::Assurance> {
        let info = self.info().ok()?;
        Some(crate::assurance::assess(
            &info,
            self.assurance_observations(),
            &self.provenance(),
        ))
    }
    /// Leave the peaks the instrument flags as reference or background ions out of the spectra
    /// read from now on (Thermo `.raw`: what ThermoRawFileParser and ProteoWizard before
    /// 3.0.20286 write). Returns false when the format flags no peaks (nothing changes).
    fn exclude_flagged_peaks(&mut self, exclude: bool) -> bool {
        let _ = exclude;
        false
    }
    /// True for the `--strict` wrapper ([`crate::strict::StrictDataset`]): outputs built from
    /// it withhold the fields its assurance does not validate (`Assurance::strict_withholds`).
    fn is_strict(&self) -> bool {
        false
    }
}

/// Read the spectrum with instrument scan number `scan` of run `run`: through
/// [`Dataset::find_spectrum`] when the reader has a scan-number map, otherwise assuming scan
/// numbers are contiguous from the first spectrum's.
pub fn spectrum_by_scan(
    ds: &mut dyn Dataset,
    run: u32,
    scan: u64,
    view: SpectrumView,
) -> Result<Spectrum> {
    if let Some(i) = ds.find_spectrum(run, scan)? {
        return ds.read_spectrum_view(run, i, view);
    }
    let first = ds.read_spectrum_view(run, 0, view)?;
    let i = scan.checked_sub(first.scan_number).ok_or_else(|| {
        Error::Usage(format!(
            "scan {scan} precedes the first scan ({})",
            first.scan_number
        ))
    })?;
    let sp = if i == 0 {
        first
    } else {
        ds.read_spectrum_view(run, i, view)?
    };
    if sp.scan_number != scan {
        return Err(Error::Usage(format!(
            "scan numbers are not contiguous; spectrum {i} is scan {}. Use --spectrum (the zero-based index)",
            sp.scan_number
        )));
    }
    Ok(sp)
}

/// Read the `nth` (1-based) spectrum of MS level `ms_level` in run `run`. MS1 and MS/MS scans
/// are interleaved in most runs, so the first MS2 scan cannot be found by assuming the MS1 scans
/// come first: the reader's per-spectrum MS levels ([`Dataset::spectrum_ms_levels`]) locate it
/// and only that spectrum is decoded; a reader without them is walked from its first spectrum,
/// `scan_count` (from `info` → `spectra[run].scan_count`) bounding the walk.
pub fn spectrum_by_level(
    ds: &mut dyn Dataset,
    run: u32,
    ms_level: u32,
    nth: u64,
    scan_count: u64,
    view: SpectrumView,
) -> Result<Spectrum> {
    if nth == 0 {
        return Err(Error::Usage(
            "--nth counts from 1 (the first scan of that level)".into(),
        ));
    }
    let mut seen = 0u64;
    if let Some(levels) = ds.spectrum_ms_levels(run)? {
        for (i, l) in levels.iter().enumerate() {
            if *l == ms_level {
                seen += 1;
                if seen == nth {
                    return ds.read_spectrum_view(run, i as u64, view);
                }
            }
        }
        return Err(Error::Usage(format!(
            "run {run} has {seen} MS{ms_level} spectra (asked for number {nth}); `info` → spectra[].extra.ms_level_counts lists the counts"
        )));
    }
    for i in 0..scan_count {
        let sp = ds.read_spectrum_view(run, i, view)?;
        if sp.ms_level == ms_level {
            seen += 1;
            if seen == nth {
                return Ok(sp);
            }
        }
    }
    Err(Error::Usage(format!(
        "run {run} has {seen} MS{ms_level} spectra (asked for number {nth}); `info` → spectra[].extra.ms_level_counts lists the counts"
    )))
}

/// Default number of per-frame records `info --view full` embeds per image (`--max-frames -1` lifts
/// it).
pub const DEFAULT_FRAME_RECORDS: usize = 100;

/// Store each image's per-frame records under `extra.frames` (plus `extra.frame_records_total`
/// and, when capped, `extra.frames_truncated: true`). Images without records are left untouched.
pub fn attach_frames(ds: &dyn Dataset, info: &mut FileInfo, limit: Option<usize>) -> Result<()> {
    for img in &mut info.images {
        let (total, records) = ds.frames(img.index, limit)?;
        if records.is_empty() {
            continue;
        }
        let truncated = (records.len() as u64) < total;
        img.extra
            .insert("frames".into(), serde_json::Value::Array(records));
        img.extra
            .insert("frame_records_total".into(), serde_json::Value::from(total));
        if truncated {
            img.extra
                .insert("frames_truncated".into(), serde_json::Value::Bool(true));
        }
    }
    Ok(())
}

/// A format implementation.
pub trait FormatReader: Send + Sync {
    /// Static description of the format (id, name, extensions, known gaps).
    fn descriptor(&self) -> FormatDescriptor;
    /// Inspect the first bytes (up to 64 KiB) and the path; return `None` if this is not ours.
    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection>;
    /// Open for reading.
    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>>;
    /// Like [`sniff`](FormatReader::sniff), for an [`Input`] that may not be on the local file
    /// system. Readers whose detection looks at other files (a directory's contents, a
    /// sibling) override it to look through [`Input::fs`]; the default calls `sniff` with the
    /// input's path.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        self.sniff(head, input.path())
    }
    /// Open an [`Input`]: a local path, a buffer in memory, or a source the host provides.
    /// Readers that read through [`Input::fs`] override it (and make `open` a thin wrapper);
    /// the default opens local inputs by path and reports every other source as unsupported
    /// (exit 6), so a reader that does not read through byte sources never touches the local disk
    /// for a buffer that merely has the same name.
    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        if input.is_local() {
            return self.open(input.path());
        }
        let d = self.descriptor();
        Err(Error::Unsupported {
            format: "input",
            feature: format!(
                "reading {} ({}) from an in-memory or host-provided source",
                d.name, d.id
            ),
            hint: Some(
                "This reader opens local files only so far: save the data to a file and open its path."
                    .into(),
            ),
        })
    }
    /// True when [`open_input`](FormatReader::open_input) reads any [`Input`] (memory, host
    /// callbacks), not only local paths.
    fn reads_any_source(&self) -> bool {
        false
    }
    /// The reader's assurance profile (its crate's `src/assurance.rs`): how to fingerprint a
    /// file's variant and which variants the development corpus validates. `None` (the
    /// default) makes every file of the format `unvalidated`.
    fn assurance(&self) -> Option<&'static crate::assurance::AssuranceProfile> {
        None
    }
}

/// Number of leading bytes handed to `sniff`.
pub const SNIFF_LEN: usize = 64 * 1024;

/// Ordered collection of readers. First definite match wins; otherwise best confidence.
pub struct Registry {
    readers: Vec<Box<dyn FormatReader>>,
    /// `--strict`: datasets opened through this registry refuse outputs their file's
    /// assurance does not validate ([`crate::strict`]).
    strict: bool,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<String> = self.readers.iter().map(|r| r.descriptor().id).collect();
        f.debug_struct("Registry")
            .field("readers", &ids)
            .field("strict", &self.strict)
            .finish()
    }
}

impl Default for Registry {
    /// An empty registry, strict when the process-wide default says so
    /// ([`crate::assurance::default_strict`]: `--strict`, `OPENREADOUT_STRICT=1`).
    fn default() -> Self {
        Registry {
            readers: Vec::new(),
            strict: crate::assurance::default_strict(),
        }
    }
}

impl Registry {
    /// An empty registry. Readers are consulted in the order they are added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a reader at the end of the detection order (and make its assurance profile known).
    pub fn register(&mut self, reader: Box<dyn FormatReader>) {
        if let Some(p) = reader.assurance() {
            crate::assurance::register_profile(p);
        }
        self.readers.push(reader);
    }

    /// Strict mode (`--strict`, MCP `strict: true`): every dataset this registry opens refuses,
    /// with exit 6 and a hint, to return outputs its file's assurance does not validate
    /// (`assurance.strict_refuses`: an unvalidated variant, an undecoded structure or an unapplied
    /// calibration affecting them). `check` and `info --view structure` are never refused.
    #[must_use]
    pub fn strict(mut self, on: bool) -> Self {
        self.strict = on;
        self
    }

    /// True in strict mode.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Builder form of [`Registry::register`].
    pub fn with(mut self, reader: Box<dyn FormatReader>) -> Self {
        self.register(reader);
        self
    }

    /// Descriptors of every registered reader, in detection order.
    pub fn descriptors(&self) -> Vec<FormatDescriptor> {
        self.readers.iter().map(|r| r.descriptor()).collect()
    }

    /// The reader with this format id, if registered.
    pub fn by_id(&self, id: &str) -> Option<&dyn FormatReader> {
        self.readers
            .iter()
            .find(|r| r.descriptor().id == id)
            .map(AsRef::as_ref)
    }

    /// Read the head of the file and ask every reader. A directory (a vendor "folder format"
    /// such as a Bruker `.d`) is offered to every reader with an empty head; readers that accept
    /// directories recognise them by their path and contents.
    pub fn detect(&self, path: &Path) -> Result<(&dyn FormatReader, Detection)> {
        self.detect_input(&Input::local(path))
    }

    /// Like [`detect`](Registry::detect), for any [`Input`] (a local path, a buffer, a host
    /// source).
    pub fn detect_input(&self, input: &Input) -> Result<(&dyn FormatReader, Detection)> {
        let path = input.path();
        let meta = input.metadata()?;
        let mut head = Vec::new();
        if !meta.is_dir() {
            let mut file = input.open()?;
            head = vec![0u8; SNIFF_LEN];
            let mut filled = 0;
            while filled < head.len() {
                let n = file
                    .read(&mut head[filled..])
                    .map_err(|e| Error::io(path, e))?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            head.truncate(filled);
            if filled == 0 {
                // An empty file is a truncated file (interrupted copy or acquisition), not an
                // unknown format: report it as corrupt so agents do not go looking for a reader.
                return Err(Error::Corrupt {
                    format: "input",
                    detail: "the file is empty (0 bytes)".into(),
                    offset: Some(0),
                });
            }
        }
        let mut best: Option<(&dyn FormatReader, Detection)> = None;
        for r in &self.readers {
            if let Some(d) = r.sniff_input(&head, input) {
                let better = match &best {
                    None => true,
                    Some((_, b)) => rank(d.confidence) > rank(b.confidence),
                };
                if better {
                    let definite = d.confidence == DetectConfidence::Definite;
                    best = Some((r.as_ref(), d));
                    if definite {
                        break;
                    }
                }
            }
        }
        best.ok_or_else(|| Error::UnknownFormat {
            path: path.to_path_buf(),
        })
    }

    /// Detect and open.
    pub fn open(&self, path: &Path) -> Result<(Detection, Box<dyn Dataset>)> {
        let (reader, det) = self.detect(path)?;
        let ds = reader
            .open(path)
            .map_err(|e| extension_only_context(e, &det))?;
        Ok((det, self.guard(ds)?))
    }

    /// In strict mode, wrap `ds` so it refuses unvalidated outputs; otherwise return it as is.
    pub fn guard(&self, ds: Box<dyn Dataset>) -> Result<Box<dyn Dataset>> {
        if self.strict {
            crate::strict::StrictDataset::wrap(ds)
        } else {
            Ok(ds)
        }
    }

    /// Detect and open any [`Input`]. Readers that only read local files report other inputs
    /// as unsupported (exit 6).
    pub fn open_input(&self, input: &Input) -> Result<(Detection, Box<dyn Dataset>)> {
        let (reader, det) = self.detect_input(input)?;
        let ds = reader
            .open_input(input)
            .map_err(|e| extension_only_context(e, &det))?;
        Ok((det, self.guard(ds)?))
    }

    /// Detect from bytes already in memory (the first up to [`SNIFF_LEN`] bytes of a stream,
    /// e.g. standard input). `name` stands in for the path readers may consult for an extension.
    pub fn detect_bytes(&self, head: &[u8], name: &Path) -> Result<(&dyn FormatReader, Detection)> {
        let head = &head[..head.len().min(SNIFF_LEN)];
        let mut best: Option<(&dyn FormatReader, Detection)> = None;
        for r in &self.readers {
            if let Some(d) = r.sniff(head, name)
                && best
                    .as_ref()
                    .is_none_or(|(_, b)| rank(d.confidence) > rank(b.confidence))
            {
                let definite = d.confidence == DetectConfidence::Definite;
                best = Some((r.as_ref(), d));
                if definite {
                    break;
                }
            }
        }
        best.ok_or_else(|| Error::UnknownFormat {
            path: name.to_path_buf(),
        })
    }
}

/// A reader chosen only for the file's extension that then fails to open it says so: the file
/// may be another kind of file with that extension (a JSON model named `.emd`, a Java object
/// stream named `.ser`, a library catalogue named `.mrc`), not a damaged one.
fn extension_only_context(e: Error, det: &Detection) -> Error {
    if det.confidence != DetectConfidence::ExtensionOnly {
        return e;
    }
    match e {
        Error::Corrupt {
            format,
            detail,
            offset,
        } => Error::Corrupt {
            format,
            detail: format!("{detail} ({})", crate::error::EXTENSION_ONLY),
            offset,
        },
        Error::Unsupported {
            format,
            feature,
            hint,
        } => Error::Unsupported {
            format,
            feature: format!("{feature} ({})", crate::error::EXTENSION_ONLY),
            hint: Some(match hint {
                Some(h) => format!("{} {h}", crate::error::EXTENSION_ONLY_HINT),
                None => crate::error::EXTENSION_ONLY_HINT.into(),
            }),
        },
        other => other,
    }
}

fn rank(c: DetectConfidence) -> u8 {
    match c {
        DetectConfidence::Definite => 3,
        DetectConfidence::Likely => 2,
        DetectConfidence::ExtensionOnly => 1,
    }
}

/// Helper for readers: does the path have one of these extensions (case-insensitive)?
pub fn has_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Confidence;

    #[derive(Debug)]
    struct PathOnly;

    impl FormatReader for PathOnly {
        fn descriptor(&self) -> FormatDescriptor {
            FormatDescriptor {
                id: "path-only".into(),
                name: "Path-only test format".into(),
                vendor: String::new(),
                extensions: vec!["po".into()],
                family: "test".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: Vec::new(),
            }
        }
        fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
            head.starts_with(b"PO").then_some(Detection {
                format_id: "path-only",
                confidence: DetectConfidence::Definite,
                note: None,
            })
        }
        fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
            Err(Error::Other(format!("opened {}", path.display())))
        }
    }

    #[test]
    fn unmigrated_readers_refuse_non_file_inputs() {
        let reg = Registry::new().with(Box::new(PathOnly));
        let input = Input::from_bytes("x.po", b"PO data".to_vec());
        let (r, det) = reg.detect_input(&input).unwrap();
        assert_eq!(det.format_id, "path-only");
        assert!(!r.reads_any_source());
        let err = reg.open_input(&input).map(|_| ()).unwrap_err();
        assert_eq!(err.exit_code(), 6, "{err}");
        assert!(err.hint().is_some());
        // Local paths still reach `open`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.po");
        std::fs::write(&path, b"PO data").unwrap();
        let err = reg
            .open_input(&Input::local(&path))
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().starts_with("opened "), "{err}");
        // Empty inputs are corrupt, missing ones are I/O errors, like `detect`.
        let empty = Input::from_bytes("e.po", Vec::new());
        let code = |i: &Input| reg.detect_input(i).map(|_| ()).unwrap_err().exit_code();
        assert_eq!(code(&empty), 4);
        assert_eq!(code(&empty.with_path("other.po")), 5);
    }

    /// Matches `.ext` files by name only and finds them corrupt.
    #[derive(Debug)]
    struct ByName;

    impl FormatReader for ByName {
        fn descriptor(&self) -> FormatDescriptor {
            FormatDescriptor {
                extensions: vec!["ext".into()],
                ..PathOnly.descriptor()
            }
        }
        fn sniff(&self, _head: &[u8], path: &Path) -> Option<Detection> {
            (path.extension()? == "ext").then_some(Detection {
                format_id: "by-name",
                confidence: DetectConfidence::ExtensionOnly,
                note: None,
            })
        }
        fn open(&self, _path: &Path) -> Result<Box<dyn Dataset>> {
            Err(Error::corrupt("by-name", "no signature"))
        }
    }

    #[test]
    fn extension_only_failures_say_only_the_extension_matched() {
        let reg = Registry::new().with(Box::new(ByName));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.ext");
        std::fs::write(&path, b"{\"json\": true}").unwrap();
        let err = reg.open(&path).map(|_| ()).unwrap_err();
        assert_eq!(err.exit_code(), 4);
        assert!(
            err.to_string().contains("only the file extension matched"),
            "{err}"
        );
        assert!(err.hint().unwrap().contains("another kind of file"));
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;
    use crate::model::{CheckReport, FileInfo, LsEntry, Spectrum};

    /// A run with the given MS levels; counts the spectra decoded.
    struct Run {
        levels: Vec<u32>,
        indexed: bool,
        reads: u64,
    }
    impl Dataset for Run {
        fn info(&self) -> Result<FileInfo> {
            Err(Error::Other("unused".into()))
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Err(Error::Other("unused".into()))
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("fake", "fake"))
        }
        fn read_spectrum(&mut self, _: u32, i: u64) -> Result<Spectrum> {
            self.reads += 1;
            Ok(Spectrum {
                index: i,
                ms_level: self.levels[i as usize],
                ..Spectrum::default()
            })
        }
        fn spectrum_ms_levels(&mut self, _: u32) -> Result<Option<Vec<u32>>> {
            Ok(self.indexed.then(|| self.levels.clone()))
        }
    }

    #[test]
    fn nth_of_level_uses_the_index_when_there_is_one() {
        for indexed in [true, false] {
            let mut r = Run {
                levels: vec![1, 2, 2, 1, 2],
                indexed,
                reads: 0,
            };
            let sp = spectrum_by_level(&mut r, 0, 2, 3, 5, SpectrumView::Primary).unwrap();
            assert_eq!(sp.index, 4);
            assert_eq!(r.reads, if indexed { 1 } else { 5 });
            let err = spectrum_by_level(&mut r, 0, 2, 4, 5, SpectrumView::Primary).unwrap_err();
            assert!(err.to_string().contains("has 3 MS2 spectra"), "{err}");
        }
    }
}
