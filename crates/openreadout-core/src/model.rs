//! The normalized metadata model. Field names follow the OME data model wherever one
//! exists, so agents can rely on a single vocabulary across vendors.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::pixel::PixelType;
use crate::provenance::{Confidence, ProvenanceMap};

/// Static description of a format as the tool understands it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormatDescriptor {
    /// Short id used on the command line, e.g. `czi`.
    pub id: String,
    /// Human name, e.g. `Zeiss CZI`.
    pub name: String,
    /// Vendor (nominative use only).
    pub vendor: String,
    /// File extensions, lowercase, without the dot.
    pub extensions: Vec<String>,
    /// Family: `microscopy`, `mass-spectrometry`, `flow-cytometry`, ...
    pub family: String,
    /// True when the tool can read this format.
    pub can_read: bool,
    /// True when `export` can also write this format (e.g. mzML, OME-Zarr).
    pub can_write: bool,
    /// Overall confidence in this reader.
    pub confidence: Confidence,
    /// Things this reader knowingly does not handle yet.
    pub known_gaps: Vec<String>,
}

/// Physical pixel size in micrometres (the OME default unit). Every present value is > 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PhysicalSize {
    /// Pixel width in µm (the distance between neighbouring columns).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Pixel height in µm (the distance between neighbouring rows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Spacing between Z planes (focal slices) in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z: Option<f64>,
    /// Always `µm` in this schema version.
    pub unit: String,
}

impl PhysicalSize {
    /// Sizes in µm. A size that is not a positive finite number (files store 0 for an axis
    /// that was never calibrated, e.g. Y of a line scan or Z of a single plane) is dropped:
    /// missing information is omitted, never encoded as 0 (`book/src/guides/metadata.md`).
    pub fn micrometres(x: Option<f64>, y: Option<f64>, z: Option<f64>) -> Self {
        let keep = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0);
        PhysicalSize {
            x: keep(x),
            y: keep(y),
            z: keep(z),
            unit: "µm".into(),
        }
    }
    /// True when no axis is calibrated.
    pub fn is_empty(&self) -> bool {
        self.x.is_none() && self.y.is_none() && self.z.is_none()
    }
}

/// One acquisition channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelInfo {
    /// Zero-based channel index (the `c` of a plane).
    pub index: u32,
    /// Channel name as the acquisition software shows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Fluorescent dye or protein imaged in this channel (e.g. `DAPI`, `GFP`), if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fluorophore: Option<String>,
    /// Excitation wavelength in nanometres: the light used to make the sample fluoresce.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excitation_nm: Option<f64>,
    /// Emission wavelength in nanometres: the light collected from the sample. What the file
    /// records varies by format (a dye's emission peak, a filter's centre, the start of a
    /// spectral detection window for Leica λ scans); `docs/formats/<fmt>.md` says which. When
    /// a detection band is known, `emission_band_*_nm` give it explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission_nm: Option<f64>,
    /// Detection band `[start_nm, end_nm]` when the instrument records a range instead of a single emission wavelength.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission_range_nm: Option<[f64; 2]>,
    /// Short-wavelength edge of the detection band in nanometres (spectral detector window or
    /// emission filter), when the file records the band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission_band_start_nm: Option<f64>,
    /// Long-wavelength edge of the detection band in nanometres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission_band_end_nm: Option<f64>,
    /// Centre of the detection band in nanometres: (start + end) / 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission_band_center_nm: Option<f64>,
    /// Display colour as `#RRGGBB` if the file records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Acquisition mode as a readable label, `<technique>[ <contrast>]` (e.g. `Laser Scanning
    /// Confocal`, `Widefield Fluorescence`, `Brightfield`, `Phase Contrast`); OME enumeration
    /// tokens (`WideField`) are turned into these labels, other vendor wording is kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition_mode: Option<String>,
    /// Exposure (integration) time of the detector in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_ms: Option<f64>,
}

/// The objective lens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectiveInfo {
    /// Objective model name as the vendor records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Nominal magnification (e.g. `63` for a 63× lens).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nominal_magnification: Option<f64>,
    /// Numerical aperture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_na: Option<f64>,
    /// Immersion medium between lens and sample (`Oil`, `Water`, `Air`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub immersion: Option<String>,
}

/// The instrument and software that produced the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InstrumentInfo {
    /// Instrument manufacturer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Instrument model (e.g. microscope stand or mass spectrometer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Acquisition software.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    /// Version of the acquisition software.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,
    /// Detector (camera or photomultiplier) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detector: Option<String>,
}

/// Mosaic (tiled/stitched) acquisition summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MosaicInfo {
    /// Number of tiles (fields of view) in the mosaic.
    pub tile_count: u32,
    /// Width of one tile in pixels, when all tiles share it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_width: Option<u32>,
    /// Height of one tile in pixels, when all tiles share it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_height: Option<u32>,
    /// Whether the reader stitches tiles into a single plane on read.
    pub stitched_on_read: bool,
}

/// One image (a scene, series, or position) inside the file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageInfo {
    /// Zero-based index used by `--image`.
    pub index: u32,
    /// Image (scene, series or position) name, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Width in pixels.
    pub size_x: u32,
    /// Height in pixels.
    pub size_y: u32,
    /// Number of Z planes (focal slices).
    pub size_z: u32,
    /// Number of channels.
    pub size_c: u32,
    /// Number of time points.
    pub size_t: u32,
    /// Storage order of planes, e.g. `XYCZT` (X and Y always first).
    pub dimension_order: String,
    /// Sample type of every plane.
    pub pixel_type: PixelType,
    /// 1 for grayscale channels, 3 for interleaved RGB.
    pub samples_per_pixel: u32,
    /// Physical size of one pixel (and the Z step).
    pub physical_size: PhysicalSize,
    /// Interval between time points in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_increment_s: Option<f64>,
    /// One entry per channel, in channel order.
    pub channels: Vec<ChannelInfo>,
    /// The objective lens, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<ObjectiveInfo>,
    /// Instrument and software, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<InstrumentInfo>,
    /// ISO-8601 acquisition start, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquired_at: Option<String>,
    /// Tile layout when the image was acquired as a mosaic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mosaic: Option<MosaicInfo>,
    /// Number of resolution levels stored (1 = no pyramid).
    pub pyramid_levels: u32,
    /// Size, downsampling and stored tile size of every resolution level, level 0 first. Filled
    /// by readers of pyramidal or tiled images (read a level with `--level N`, a rectangle
    /// with `--region X,Y,W,H`); empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolution_levels: Vec<crate::region::ResolutionLevel>,
    /// `size_z * size_c * size_t`.
    pub plane_count: u64,
    /// Format-specific extras that have no OME equivalent, in our own vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ImageInfo {
    /// A blank image with the given geometry; callers fill the rest.
    pub fn new(index: u32, size_x: u32, size_y: u32, pixel_type: PixelType) -> Self {
        ImageInfo {
            index,
            name: None,
            size_x,
            size_y,
            size_z: 1,
            size_c: 1,
            size_t: 1,
            dimension_order: "XYCZT".into(),
            pixel_type,
            samples_per_pixel: 1,
            physical_size: PhysicalSize::micrometres(None, None, None),
            time_increment_s: None,
            channels: Vec::new(),
            objective: None,
            instrument: None,
            acquired_at: None,
            mosaic: None,
            pyramid_levels: 1,
            resolution_levels: Vec::new(),
            plane_count: 1,
            extra: BTreeMap::new(),
        }
    }

    /// Recompute `plane_count` from the dimensions, and fill each channel's explicit
    /// detection band (`emission_band_*_nm`) from `emission_range_nm`.
    pub fn finish(mut self) -> Self {
        // Saturating: a corrupt header can declare sizes whose product exceeds u64.
        self.plane_count = u64::from(self.size_z)
            .saturating_mul(u64::from(self.size_c))
            .saturating_mul(u64::from(self.size_t));
        for c in &mut self.channels {
            c.fill_band();
            // One readable vocabulary for every reader (OME tokens → labels).
            c.acquisition_mode = c
                .acquisition_mode
                .as_deref()
                .and_then(crate::acquisition_mode::label);
        }
        // Unit conversions leave binary noise (1e-7 m × 1e6 = 0.09999999999999999 µm): keep
        // 15 significant digits, which no instrument records more precisely.
        for v in [
            &mut self.physical_size.x,
            &mut self.physical_size.y,
            &mut self.physical_size.z,
            &mut self.time_increment_s,
        ] {
            *v = v.map(round_noise);
        }
        if let Some(o) = &mut self.objective {
            o.lens_na = o.lens_na.map(round_noise);
            o.nominal_magnification = o.nominal_magnification.map(round_noise);
        }
        // Text fields are compared and exported as values: no padding, no blanks.
        if let Some(o) = &mut self.objective {
            o.model = trimmed(o.model.take());
            o.immersion = o.immersion.as_deref().and_then(immersion_label);
        }
        if let Some(i) = &mut self.instrument {
            for s in [
                &mut i.manufacturer,
                &mut i.model,
                &mut i.software,
                &mut i.software_version,
                &mut i.detector,
            ] {
                *s = trimmed(s.take());
            }
        }
        self
    }
}

/// `v` to 15 significant digits: removes the last-bit noise of decimal conversions
/// (`0.30000000000000004` → `0.3`). Non-finite values are returned unchanged.
pub fn round_noise(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    format!("{v:.14e}").parse().unwrap_or(v)
}

fn trimmed(s: Option<String>) -> Option<String> {
    let s = s?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else if t.len() == s.len() {
        Some(s)
    } else {
        Some(t.to_string())
    }
}

/// An objective's immersion medium in one spelling for every reader: `Oil`, `Water`, `Air`
/// (also for `dry`), `Glycerol`, `Multi` or `Other`, matched without case (`DRY` → `Air`);
/// other media (`Silicone`) are kept as written, trimmed. `None` for blank text.
pub(crate) fn immersion_label(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    Some(
        match t.to_ascii_lowercase().as_str() {
            "oil" => "Oil",
            "water" => "Water",
            "air" | "dry" => "Air",
            "glycerol" | "glycerin" | "glycerine" => "Glycerol",
            "multi" => "Multi",
            "other" => "Other",
            _ => t,
        }
        .to_string(),
    )
}

impl ChannelInfo {
    /// Set the detection band: `emission_range_nm` and `emission_band_{start,end,center}_nm`.
    /// Ignored unless both edges are finite, positive and ordered.
    pub fn set_band(&mut self, start_nm: f64, end_nm: f64) {
        if start_nm.is_finite() && end_nm.is_finite() && start_nm > 0.0 && end_nm >= start_nm {
            self.emission_range_nm = Some([start_nm, end_nm]);
            self.emission_band_start_nm = Some(start_nm);
            self.emission_band_end_nm = Some(end_nm);
            self.emission_band_center_nm = Some(f64::midpoint(start_nm, end_nm));
        }
    }

    /// Fill `emission_band_*_nm` from `emission_range_nm` when a reader set only the range.
    pub fn fill_band(&mut self) {
        if self.emission_band_start_nm.is_none()
            && let Some([a, b]) = self.emission_range_nm
        {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            if lo.is_finite() && hi.is_finite() {
                self.emission_band_start_nm = Some(lo);
                self.emission_band_end_nm = Some(hi);
                self.emission_band_center_nm = Some(f64::midpoint(lo, hi));
            }
        }
    }
}

/// Output of `info` (and the `file` part of `info --view full`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileInfo {
    /// The path as given by the caller.
    pub path: String,
    /// Size in bytes (for directory formats such as Bruker `.d`, the files that were read).
    pub size_bytes: u64,
    /// The format that read the file.
    pub format: FormatDescriptor,
    /// Version of the container format as written in the file, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_version: Option<String>,
    /// Images in the file (empty for tables, spectra and traces).
    pub images: Vec<ImageInfo>,
    /// Tabular datasets (flow cytometry events, ...). Empty for image formats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<TableInfo>,
    /// Mass-spectrometry runs. Empty for other formats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectra: Vec<SpectraInfo>,
    /// Sampled-signal blocks (electrophysiology, chromatography, NMR). Empty for other formats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<TraceInfo>,
    /// Total planes across all images.
    pub plane_count: u64,
    /// Notes the reader wants the caller to see (e.g. "pyramid levels skipped").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Output of `info --view full`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Dump {
    /// What `info` reports, including the derived `experiment`.
    pub file: crate::experiment::InfoOutput,
    /// The vendor's own metadata tree, converted to JSON without renaming anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<serde_json::Value>,
    /// Where each normalized field came from. Keys are JSON paths into `file`.
    pub provenance: ProvenanceMap,
}

/// A file embedded in the container (thumbnail, label image, time stamps, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentInfo {
    /// Zero-based index, usable with `export --attachment #<index>`.
    pub index: u32,
    /// Name as stored in the container (e.g. `Thumbnail`, `Label`).
    pub name: String,
    /// Content type as the file declares it (e.g. `JPG`, `CZI`, `CZTIMS`).
    pub content_type: String,
    /// Suggested file extension for the extracted bytes, without the dot.
    pub extension: String,
    /// Byte offset of the attachment payload in its file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Payload size in bytes.
    pub size: u64,
    /// Format-specific details in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Output of `export --attachment`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// The attachment that was extracted.
    pub attachment: AttachmentInfo,
    /// Where the bytes were written.
    pub output: String,
    /// Number of bytes written.
    pub bytes_written: u64,
    /// xxh3-128 of the written bytes, 32 hex chars.
    pub xxh3: String,
    /// True once the output was read back and matched.
    pub verified: bool,
}

/// One row of `info --view structure`: a structural element of the container.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LsEntry {
    /// `image`, `plane`, `segment`, `chunk`, `block`, `attachment`, `metadata`, `pyramid-level`, ...
    pub kind: String,
    /// Element name (an id, a chunk name or a path inside the container).
    pub name: String,
    /// Byte offset of the element in the file, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Size of the element in bytes, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Which image this element belongs to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<u32>,
    /// Format-specific details (JSON), omitted when empty.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

/// Output of `info --view structure`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Listing {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Container elements in file order.
    pub entries: Vec<LsEntry>,
    /// Present when the file is not finished: still being written (`in_progress`) or
    /// stopped before the end (`interrupted`). See `book/src/guides/lab-shares.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<crate::live::Acquisition>,
}

/// Severity of a `check` finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Severity {
    /// Worth knowing; the file is fine.
    Info,
    /// Something unusual that does not stop the file from being read.
    Warning,
    /// The file is damaged or inconsistent; some data may be unreadable.
    Error,
}

impl Severity {
    /// The lowercase name used in JSON (`info`, `warning`, `error`).
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One `check` finding.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// How serious the finding is.
    pub severity: Severity,
    /// Stable code, e.g. `truncated`, `bad_offset`, `missing_planes`, `directory_mismatch`.
    pub code: String,
    /// Plain-English description.
    pub message: String,
    /// Byte offset the finding refers to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

impl Finding {
    /// A finding of severity `error` with a stable `code`.
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            offset: None,
        }
    }
    /// A finding of severity `warning` with a stable `code`.
    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Warning,
            code: code.into(),
            message: message.into(),
            offset: None,
        }
    }
    /// A finding of severity `info` with a stable `code`.
    pub fn info(code: &str, message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Info,
            code: code.into(),
            message: message.into(),
            offset: None,
        }
    }
    /// Attach the byte offset the finding refers to.
    pub fn at(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// Output of `check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckReport {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// True when no finding has severity `error`.
    pub ok: bool,
    /// What was verified, in plain words, so the caller knows what "ok" covers.
    pub checks_performed: Vec<String>,
    /// Everything found, in the order checked.
    pub findings: Vec<Finding>,
    /// Present when the file is not finished: still being written (`in_progress`: findings
    /// about the unfinished tail are warnings and `ok` covers the complete part) or stopped
    /// before the end (`interrupted`). See `book/src/guides/lab-shares.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<crate::live::Acquisition>,
    /// Whether the file lies inside what its reader has been validated on (the same block as
    /// `info` → `assurance`). `ok` says the file is intact; `assurance` says whether its values
    /// can be trusted. Filled by the `check` command when the header can be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<crate::assurance::Assurance>,
}

impl CheckReport {
    /// An empty report that is `ok` until an error finding is pushed.
    pub fn new(path: impl Into<String>, format: impl Into<String>) -> Self {
        CheckReport {
            path: path.into(),
            format: format.into(),
            ok: true,
            checks_performed: Vec::new(),
            findings: Vec::new(),
            acquisition: None,
            assurance: None,
        }
    }
    /// Record a check that was carried out (shown under `checks_performed`).
    pub fn performed(&mut self, what: impl Into<String>) {
        self.checks_performed.push(what.into());
    }
    /// Add a finding; an `error` finding makes the report not `ok`.
    pub fn push(&mut self, f: Finding) {
        if f.severity == Severity::Error {
            self.ok = false;
        }
        self.findings.push(f);
    }
}

/// Output of `self formats`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormatsOutput {
    /// Every registered format, in detection order.
    pub formats: Vec<FormatDescriptor>,
}

/// Output of `info --view format`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DetectOutput {
    /// The input file.
    pub path: String,
    /// Format id, e.g. `czi`.
    pub format: String,
    /// Human name of the format, e.g. `Zeiss CZI`.
    pub name: String,
    /// How the format was recognised.
    pub confidence: DetectConfidence,
    /// Why detection is less than definite, if it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How sure detection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DetectConfidence {
    /// A format-specific signature matched.
    Definite,
    /// Structure looks right but no unique signature exists.
    Likely,
    /// Only the file extension matched.
    ExtensionOnly,
}

/// One row of `check --planes`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaneHash {
    /// Image index.
    pub image: u32,
    /// Pyramid level (0 = full resolution); omitted when 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub level: u32,
    /// The rectangle read (level pixel coordinates) when `--region` was given; the hash and
    /// `width`/`height` are then those of the region. Absent = the whole plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::region::Region>,
    /// Channel index.
    pub c: u32,
    /// Z index (focal plane).
    pub z: u32,
    /// Time index.
    pub t: u32,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Sample type.
    pub pixel_type: PixelType,
    /// 1 for grayscale, 3 for interleaved RGB.
    pub samples_per_pixel: u32,
    /// xxh3-128 of the raw little-endian sample bytes, 32 hex chars.
    pub xxh3: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Output of `check --planes`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlanesOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// One row per plane, in the order read.
    pub planes: Vec<PlaneHash>,
    /// Present when the file is not finished; while it is `in_progress` only complete planes
    /// are read. See `book/src/guides/lab-shares.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<crate::live::Acquisition>,
}

// ---------- non-image data (flow cytometry events, mass-spectrometry spectra, ...) ----------

/// A column of a tabular dataset (e.g. an FCS parameter).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ColumnInfo {
    /// Zero-based column index.
    pub index: u32,
    /// Short name (FCS `$PnN`).
    pub name: String,
    /// Descriptive label (FCS `$PnS`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// NumPy-style dtype of the values as returned by `read_table` (`float32`, `float64`, `uint32`, ...).
    pub dtype: String,
    /// Physical unit of the values, if the format records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Nominal `[min, max]` range when the format declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Format-specific column metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A tabular dataset inside the file (rows × columns), e.g. FCS events.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TableInfo {
    /// Zero-based table index.
    pub index: u32,
    /// Table name, if the format has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Number of rows (e.g. recorded events).
    pub row_count: u64,
    /// One entry per column.
    pub columns: Vec<ColumnInfo>,
    /// Format-specific table metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Rows of a table, column-major: `columns[c][r]` as f64 (lossless for ≤ 32-bit inputs).
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// Table index.
    pub table: u32,
    /// Zero-based index of the first row in `columns`.
    pub first_row: u64,
    /// Column-major values: `columns[c][r]`.
    pub columns: Vec<Vec<f64>>,
}

/// A slice of a table as rows, for JSON consumers (`openreadout_table`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TableSlice {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Table index (see `info` → `tables[]`).
    pub table: u32,
    /// Zero-based index of the first row returned.
    pub first_row: u64,
    /// Rows in the whole table.
    pub total_rows: u64,
    /// Column names (FCS `$PnN`).
    pub columns: Vec<String>,
    /// Column labels (FCS `$PnS`), `null` where absent.
    pub labels: Vec<Option<String>>,
    /// Row-major values; `rows[r][c]`. Non-finite values serialize as `null`.
    pub rows: Vec<Vec<f64>>,
    /// True when more rows follow the returned slice.
    pub truncated: bool,
    /// Present when the values were processed (FCS scale values, compensation, transforms,
    /// population membership columns); absent for raw values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<crate::flow::TableProcessing>,
    /// Present with `--where`/`--count`: the row conditions and how many rows of the whole
    /// table meet them. `rows`, `first_row` and `truncated` then page through the matching rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<TableFilterSummary>,
}

/// Rows of a whole table that meet every condition (`table --where`, `--count`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TableFilterSummary {
    /// The conditions as parsed (`FITC-A > 1000`), all of which a row must meet. Empty with
    /// `--count` alone (every row counts).
    pub conditions: Vec<String>,
    /// Rows of the whole table that meet every condition (not only the returned page).
    pub matched_rows: u64,
    /// Rows in the whole table.
    pub total_rows: u64,
    /// `matched_rows / total_rows × 100` (0 for an empty table).
    pub percent: f64,
    /// The values the conditions were tested on: `raw` (as stored) or `processed` (FCS scale
    /// values after compensation/transforms; see `processing`).
    pub values: String,
}

/// Output of `export --format csv`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableExportReport {
    /// The input file.
    pub input: String,
    /// The CSV file written.
    pub output: String,
    /// Always `csv` in this schema version.
    pub format: String,
    /// Table index written.
    pub table: u32,
    /// First row written (zero-based).
    pub first_row: u64,
    /// Data rows written.
    pub rows_written: u64,
    /// Columns written.
    pub columns_written: u32,
    /// Header lines before the data (1, or 2 with `--labels`).
    pub header_lines: u32,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the written file was read back and every value parsed equal to the source.
    pub verified: bool,
}

/// Output of `export --format csv` for a trace (one sweep: a time column plus one column per channel).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceExportReport {
    /// The input file.
    pub input: String,
    /// The CSV file written.
    pub output: String,
    /// Always `csv` in this schema version.
    pub format: String,
    /// Trace index written.
    pub trace: u32,
    /// Sweep index written.
    pub sweep: u32,
    /// First sample of the sweep written (zero-based).
    pub first_sample: u64,
    /// Samples (rows) written.
    pub samples_written: u64,
    /// Signal channels written (the time column is not counted).
    pub channels_written: u32,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the written file was read back and every value parsed equal to the source.
    pub verified: bool,
}

/// A collection of mass spectra (one acquisition run).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpectraInfo {
    /// Zero-based run index.
    pub index: u32,
    /// Run name, if the file records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Number of spectra (scans) in the run.
    pub scan_count: u64,
    /// MS levels present (1 = full scans, 2 = MS/MS, ...).
    pub ms_levels: Vec<u32>,
    /// Retention-time range in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_range_s: Option<[f64; 2]>,
    /// Instrument and software, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<InstrumentInfo>,
    /// Format-specific run metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One mass spectrum.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Spectrum {
    /// Zero-based spectrum index within its run.
    pub index: u64,
    /// Scan number as the instrument counts it (1-based in most formats).
    pub scan_number: u64,
    /// MS level: 1 for a full scan, 2 for a fragment (MS/MS) scan, and so on.
    pub ms_level: u32,
    /// Retention time in seconds; null when the file states none for this spectrum (never an
    /// invented 0).
    #[serde(default)]
    pub rt_s: Option<f64>,
    /// `positive`, `negative` or `unknown`.
    pub polarity: String,
    /// True when the peaks are centroids (one point per peak) rather than a sampled profile.
    pub centroided: bool,
    /// m/z of the precursor ion that was isolated and fragmented (MS2 and above).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// Charge state of the precursor, when the instrument determined it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_charge: Option<i32>,
    /// Instrument scan filter or scan description string (e.g. Thermo `FTMS + p ESI Full ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_filter: Option<String>,
    /// Sum of all intensities in the spectrum (TIC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_ion_current: Option<f64>,
    /// The spectrum's identifier in its file (mzML `id`, e.g. `controllerType=0 controllerNumber=1 scan=17`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    /// m/z of the most intense peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_peak_mz: Option<f64>,
    /// Intensity of the most intense peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_peak_intensity: Option<f64>,
    /// Intensity of the selected precursor ion, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_intensity: Option<f64>,
    /// Precursor isolation window `[lower, upper]` in m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_window_mz: Option<[f64; 2]>,
    /// Dissociation method: `CID`, `HCD`, `ETD`, `ECD`, `ETHCD`, `UVPD`, ... (upper case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    /// Collision energy as recorded (usually eV, or % normalized for Thermo `@hcd35.00`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision_energy: Option<f64>,
    /// Ion mobility of the precursor or scan, as inverse reduced mobility 1/K0 in V·s/cm².
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inverse_reduced_mobility: Option<f64>,
    /// Scan (acquisition) window `[lower, upper]` in m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_window_mz: Option<[f64; 2]>,
    /// Format-specific scan metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
    /// Mass-to-charge ratios of the points, ascending.
    pub mz: Vec<f64>,
    /// Intensity of each point (same length as `mz`).
    pub intensity: Vec<f32>,
}

/// Output of `spectra --scan`/`--index`: one spectrum with its arrays.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Run index the spectrum belongs to.
    pub run: u32,
    /// `primary` (profile when recorded) or `centroid` (the instrument's centroid list).
    pub view: String,
    /// Number of points in the full spectrum (the arrays may be shortened by `max_points`).
    pub point_count: u64,
    /// True when `mz`/`intensity` were shortened to `max_points`.
    pub truncated: bool,
    /// The spectrum, with its arrays.
    pub spectrum: Spectrum,
}

// ---------- sampled signals (electrophysiology sweeps, chromatograms, NMR FIDs) ----------

/// One channel of a sampled-signal dataset.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SignalChannelInfo {
    /// Zero-based channel index.
    pub index: u32,
    /// Channel name as recorded.
    pub name: String,
    /// Physical unit of the scaled values (`pA`, `mV`, `AU`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// NumPy-style dtype of the raw stored samples.
    pub dtype: String,
    /// `value = raw * scale + offset` when the file stores integers.
    pub scale: f64,
    /// Added after scaling; see `scale`.
    pub offset: f64,
    /// Format-specific channel metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A block of uniformly sampled signals (a sweep, a chromatogram run, an FID).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TraceInfo {
    /// Zero-based trace index.
    pub index: u32,
    /// Trace name, if the format has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Samples per second.
    pub sample_rate_hz: f64,
    /// Samples per sweep (the longest sweep when sweeps differ; see `extra.sweep_sample_counts`).
    pub sample_count: u64,
    /// Number of sweeps/episodes stored back to back (1 for continuous recordings).
    pub sweep_count: u32,
    /// One entry per channel.
    pub channels: Vec<SignalChannelInfo>,
    /// Time of the first sample relative to the recording start, seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_s: Option<f64>,
    /// Format-specific trace metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Samples of one trace: `channels[c][i]`, scaled to physical units as f64.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Index of the first sample in `channels` within the sweep.
    pub first_sample: u64,
    /// Scaled samples: `channels[c][i]`.
    pub channels: Vec<Vec<f64>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn finish_removes_conversion_noise() {
        let mut im = ImageInfo::new(0, 1, 1, PixelType::Uint8);
        im.physical_size = PhysicalSize::micrometres(Some(1e-7 * 1e6), Some(0.1 + 0.2), None);
        im.objective = Some(ObjectiveInfo {
            lens_na: Some(0.1 * 3.0),
            ..ObjectiveInfo::default()
        });
        let im = im.finish();
        assert_eq!(im.physical_size.x, Some(0.1));
        assert_eq!(im.physical_size.y, Some(0.3));
        assert_eq!(im.objective.unwrap().lens_na, Some(0.3));
        assert_eq!(round_noise(0.123_456_789_012_345_67), 0.123_456_789_012_346);
        assert!(round_noise(f64::NAN).is_nan());
    }

    #[test]
    fn finish_normalizes_modes_immersion_and_padding() {
        let mut im = ImageInfo::new(0, 1, 1, PixelType::Uint8);
        im.channels = vec![ChannelInfo {
            acquisition_mode: Some("LaserScanningConfocalMicroscopy".into()),
            ..ChannelInfo::default()
        }];
        im.objective = Some(ObjectiveInfo {
            model: Some("HC PL APO 10x/0.40 DRY ".into()),
            immersion: Some("DRY".into()),
            ..ObjectiveInfo::default()
        });
        im.instrument = Some(InstrumentInfo {
            software: Some(" LAS X".into()),
            detector: Some("  ".into()),
            ..InstrumentInfo::default()
        });
        let im = im.finish();
        assert_eq!(
            im.channels[0].acquisition_mode.as_deref(),
            Some("Laser Scanning Confocal")
        );
        let o = im.objective.unwrap();
        assert_eq!(o.model.as_deref(), Some("HC PL APO 10x/0.40 DRY"));
        assert_eq!(o.immersion.as_deref(), Some("Air"));
        let i = im.instrument.unwrap();
        assert_eq!(i.software.as_deref(), Some("LAS X"));
        assert_eq!(i.detector, None);
        assert_eq!(immersion_label("water").as_deref(), Some("Water"));
        assert_eq!(immersion_label("Silicone").as_deref(), Some("Silicone"));
    }

    #[test]
    fn physical_size_drops_uncalibrated_axes() {
        let p = PhysicalSize::micrometres(Some(0.5), Some(0.0), Some(f64::NAN));
        assert_eq!(p.x, Some(0.5));
        assert_eq!(p.y, None);
        assert_eq!(p.z, None);
        assert!(
            PhysicalSize::micrometres(None, Some(-1.0), None)
                .y
                .is_none()
        );
    }
}
