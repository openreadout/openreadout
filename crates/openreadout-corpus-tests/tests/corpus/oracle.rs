//! The ground-truth JSON the oracle scripts write (`corpus/oracle/<id>.json`), as read here.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Oracle {
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) images: Vec<OracleImage>,
    /// Chromatogram ground truth for runs the export holds only as traces (SRM).
    #[serde(default)]
    pub(crate) chromatograms: Option<Vec<OracleChromatogram>>,
    /// Set from the manifest entry (`srm_product_tolerance`), not read from the oracle file.
    #[serde(skip)]
    pub(crate) srm_product_tolerance: Option<f64>,
    /// ND2 only: channels, acquisition start and sampled per-frame records from the `nd2` package.
    #[serde(default)]
    pub(crate) nd2_meta: Option<Nd2Meta>,
    #[serde(default)]
    pub(crate) is_legacy: bool,
    /// Tabular data sets (FCS). Compared when present.
    #[serde(default)]
    pub(crate) tables: Vec<OracleTable>,
    /// Mass-spectrometry ground truth (from the depositor's own open-format conversion).
    #[serde(default)]
    pub(crate) spectra: Option<OracleSpectra>,
    /// timsTOF frames (timsrust ground truth). Compared when present.
    #[serde(default)]
    pub(crate) tdf: Option<OracleTdf>,
    /// Sampled signals (electrophysiology) and spectra (NMR, JCAMP-DX). Compared when present.
    #[serde(default)]
    pub(crate) traces: Vec<OracleTrace>,
    /// Plate-reader exports: allotropy's ASM output summarized per detection mode (oracle/plate.py).
    #[serde(default)]
    pub(crate) plate: Option<OraclePlate>,
    /// High-content screening plates (oracle/hcs.py): layout, completeness, planes.
    #[serde(default)]
    pub(crate) hcs: Option<serde_json::Value>,
    /// `false` when no independent reader exists and the ground truth is a second implementation
    /// of our own format notes (PL2): a match is then reported as a self-consistency check.
    #[serde(default = "yes")]
    pub(crate) independent: bool,
    /// Per-frame records of image 0 (camera frame counter, absolute time), compared with
    /// `Dataset::frames` (DCIMG).
    #[serde(default)]
    pub(crate) check_frames: Option<Vec<OracleFrameRecord>>,
    /// Software that wrote a depositor export (`[id, version]` or `[type, name, version]`).
    #[serde(default)]
    pub(crate) export_software: Vec<Vec<String>>,
}

impl Oracle {
    /// The export was written by a converter that left flagged Thermo peaks out.
    pub(crate) fn export_excludes_flagged_peaks(&self) -> bool {
        openreadout_corpus_tests::export_excludes_flagged_peaks(&self.export_software)
    }
}
#[derive(Deserialize)]
pub(crate) struct OracleFrameRecord {
    pub(crate) t: u32,
    #[serde(default)]
    pub(crate) frame: Option<u64>,
    #[serde(default)]
    pub(crate) acquired_at: Option<String>,
}

pub(crate) fn yes() -> bool {
    true
}
#[derive(Deserialize)]
pub(crate) struct OraclePlate {
    pub(crate) groups: Vec<OraclePlateGroup>,
    /// Values the vendor software calculated, per mode (independent text readers that separate
    /// them); compared with our `calculated` reads when present.
    #[serde(default)]
    pub(crate) calculated_groups: Vec<OraclePlateGroup>,
    #[serde(default)]
    pub(crate) header: BTreeMap<String, String>,
}
#[derive(Deserialize)]
pub(crate) struct OraclePlateGroup {
    pub(crate) mode: String,
    pub(crate) wells: usize,
    pub(crate) values: usize,
    #[serde(default)]
    pub(crate) wavelengths: Vec<f64>,
    /// The distinct times (seconds) of a kinetic read, ascending, when the oracle reads them.
    #[serde(default)]
    pub(crate) times_s: Vec<f64>,
    pub(crate) value_xxh3: String,
}
#[derive(Deserialize)]
pub(crate) struct Nd2Meta {
    #[serde(default)]
    pub(crate) channels: Vec<OracleChannel>,
    #[serde(default)]
    pub(crate) acquired_at: Option<String>,
    #[serde(default)]
    pub(crate) frames: Vec<OracleFrame>,
}
#[derive(Deserialize)]
pub(crate) struct OracleChannel {
    #[serde(default)]
    pub(crate) name: Option<String>,
    pub(crate) color: String,
    pub(crate) excitation_nm: Option<f64>,
    pub(crate) emission_nm: Option<f64>,
    #[serde(default)]
    pub(crate) component_count: u32,
}
#[derive(Deserialize)]
pub(crate) struct OracleFrame {
    pub(crate) index: u32,
    #[serde(default)]
    pub(crate) time_ms: Option<f64>,
    #[serde(default)]
    pub(crate) stage_x_um: Option<f64>,
    #[serde(default)]
    pub(crate) stage_y_um: Option<f64>,
    #[serde(default)]
    pub(crate) stage_z_um: Option<f64>,
    /// Custom-data columns by tag id (`X`, `PFS_STATUS`, `Camera_ExposureTime1`, …).
    #[serde(default)]
    pub(crate) tags: BTreeMap<String, serde_json::Value>,
}
#[derive(Deserialize)]
pub(crate) struct OracleTrace {
    pub(crate) index: u32,
    /// Absent when the oracle could not decode the trace (metadata only).
    #[serde(default)]
    pub(crate) sweep_count: Option<u32>,
    /// Absent when the oracle could not decode the trace (metadata only).
    #[serde(default)]
    pub(crate) channel_count: Option<usize>,
    #[serde(default)]
    pub(crate) channel_names: Vec<String>,
    #[serde(default)]
    pub(crate) channel_units: Vec<String>,
    /// Per-channel `extra` values the oracle knows (probe sites, ...): numbers within 1e-6,
    /// other values equal.
    #[serde(default)]
    pub(crate) channel_extra: Vec<BTreeMap<String, serde_json::Value>>,
    /// `channel_extra` keys compared up to one constant offset per trace (the oracle measures
    /// from another origin).
    #[serde(default)]
    pub(crate) channel_extra_offset_keys: Vec<String>,
    /// Absent for spectra (no time base).
    #[serde(default)]
    pub(crate) sample_rate_hz: Option<f64>,
    /// Allowed |ours - oracle| for the sample rate (pyABF truncates rates to integers).
    /// Default: equal to 1e-9 relative.
    #[serde(default)]
    pub(crate) sample_rate_tolerance_hz: Option<f64>,
    #[serde(default)]
    pub(crate) sweeps: Vec<OracleSweep>,
    /// Trace name (NMR: `fid`, `ser`, `pdata/<n>`; JCAMP-DX: the block title).
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// Chemical-shift axis ends of a processed NMR spectrum.
    #[serde(default)]
    pub(crate) ppm_first: Option<f64>,
    #[serde(default)]
    pub(crate) ppm_last: Option<f64>,
    /// Reader parameter values (Bruker: `acqus`, `procs`, ...), compared with `extra` fields.
    #[serde(default)]
    pub(crate) parameters: Option<serde_json::Value>,
    /// The oracle failed on this trace.
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// Retention time of the first and last sample, minutes (chromatography), compared with
    /// `extra.axis` (`first`, `first + step × (n − 1)`) within 1e-6 min.
    #[serde(default)]
    pub(crate) x_first_min: Option<f64>,
    #[serde(default)]
    pub(crate) x_last_min: Option<f64>,
}
#[derive(Deserialize)]
pub(crate) struct OracleSweep {
    pub(crate) sweep: u32,
    pub(crate) sample_count: u64,
    /// When set, `xxh3` covers only the first `hashed_samples` samples (the oracle returned fewer
    /// samples than the file holds; see oracle/gen.py).
    #[serde(default)]
    pub(crate) hashed_samples: Option<u64>,
    pub(crate) channels: Vec<OracleSweepChannel>,
}
#[derive(Deserialize)]
pub(crate) struct OracleSweepChannel {
    /// xxh3-128 of the sweep's samples of this channel as little-endian f64.
    pub(crate) xxh3: String,
    /// First samples (up to 8), scaled.
    #[serde(default)]
    pub(crate) first: Vec<f64>,
}
#[derive(Deserialize)]
pub(crate) struct OracleTable {
    pub(crate) index: u32,
    pub(crate) event_count: u64,
    pub(crate) parameter_names: Vec<String>,
    #[serde(default)]
    pub(crate) dtypes: Vec<String>,
    /// xxh3-128 of all values as column-major little-endian f64; null when no oracle decoded them.
    #[serde(default)]
    pub(crate) xxh3: Option<String>,
    /// xxh3-128 of single columns (little-endian f64), keyed by our column name, for oracles that
    /// can rebuild only some columns (e.g. Neo's per-unit spike trains merged by timestamp).
    #[serde(default)]
    pub(crate) column_hashes: BTreeMap<String, String>,
    /// xxh3-128 of a column's values sorted ascending (order-free comparison), optionally only
    /// over rows where `where_column == where_value`, with the expected number of such rows.
    #[serde(default)]
    pub(crate) sorted_column_hashes: Vec<SortedColumnHash>,
    /// xxh3-128 of the rows restricted to `columns`, sorted lexicographically (order-free, but
    /// unlike `sorted_column_hashes` it keeps each row's values together).
    #[serde(default)]
    pub(crate) sorted_row_hashes: Vec<SortedRowHash>,
}
#[derive(Deserialize)]
pub(crate) struct SortedRowHash {
    pub(crate) columns: Vec<String>,
    pub(crate) count: u64,
    /// Row-major little-endian f64 of the sorted rows.
    pub(crate) xxh3: String,
}
#[derive(Deserialize)]
pub(crate) struct SortedColumnHash {
    pub(crate) column: String,
    #[serde(default)]
    pub(crate) where_column: Option<String>,
    #[serde(default)]
    pub(crate) where_value: Option<f64>,
    #[serde(default)]
    pub(crate) count: Option<u64>,
    pub(crate) xxh3: String,
}
#[derive(Deserialize)]
pub(crate) struct OracleChromatogram {
    pub(crate) id: String,
    /// `tic`, `srm` or `other`.
    pub(crate) kind: String,
    pub(crate) polarity: String,
    pub(crate) precursor_mz: Option<f64>,
    pub(crate) product_mz: Option<f64>,
    pub(crate) product_lower_offset: Option<f64>,
    pub(crate) product_upper_offset: Option<f64>,
    /// Collision energy of the transition, when the oracle recorded it.
    #[serde(default)]
    pub(crate) collision_energy: Option<f64>,
    pub(crate) n_points: usize,
    pub(crate) xxh3_intensity: String,
    pub(crate) sum_intensity: f64,
    pub(crate) first_points: Vec<[f64; 2]>,
    pub(crate) n_nonzero: usize,
    pub(crate) xxh3_time_min_nonzero: String,
    pub(crate) xxh3_intensity_nonzero: String,
}
#[derive(Deserialize)]
pub(crate) struct OracleSpectra {
    pub(crate) scan_count: u64,
    /// The oracle read this very file (mzML/mzXML): address spectra by position, not scan number.
    #[serde(default)]
    pub(crate) by_index: bool,
    pub(crate) scans: Vec<OracleScan>,
    /// Set from the manifest entry (`precursor_tolerance`), not read from the oracle file.
    #[serde(skip)]
    pub(crate) precursor_tolerance: Option<f64>,
    /// Set from the manifest entry (`peaks_not_compared`), not read from the oracle file.
    #[serde(skip)]
    pub(crate) peaks_not_compared: Option<String>,
    /// Set from the manifest entry (`mz_not_compared`), not read from the oracle file.
    #[serde(skip)]
    pub(crate) mz_not_compared: Option<String>,
    /// Set from the manifest entry (`native_id_not_compared`).
    #[serde(skip)]
    pub(crate) native_id_not_compared: Option<String>,
    /// Set from the manifest entry (`precursor_not_compared`).
    #[serde(skip)]
    pub(crate) precursor_not_compared: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct OracleScan {
    pub(crate) index: u64,
    pub(crate) scan_number: u64,
    /// The export's native id (`scanId=N`, ...), when the oracle recorded it.
    #[serde(default)]
    pub(crate) native_id: Option<String>,
    pub(crate) ms_level: u32,
    pub(crate) rt_s: Option<f64>,
    /// Rounding of the export's printed retention time (mzXML), when coarser than 1e-3 s.
    #[serde(default)]
    pub(crate) rt_tolerance_s: Option<f64>,
    /// `null` when the oracle reader does not report it (pyimzML).
    #[serde(default)]
    pub(crate) polarity: Option<String>,
    #[serde(default)]
    pub(crate) centroided: Option<bool>,
    pub(crate) filter: Option<String>,
    pub(crate) precursor_mz: Option<f64>,
    pub(crate) precursor_charge: Option<i32>,
    /// mzML: list of activation CV names; mzXML: `HCD`, `CID`, ...
    #[serde(default)]
    pub(crate) activation: Option<serde_json::Value>,
    pub(crate) n_peaks: usize,
    pub(crate) mz_bits: Option<u32>,
    pub(crate) xxh3_mz: String,
    pub(crate) xxh3_intensity: String,
    pub(crate) sum_intensity: f64,
    pub(crate) mz_min: Option<f64>,
    pub(crate) mz_max: Option<f64>,
    pub(crate) first_peaks: Vec<[f64; 2]>,
    #[serde(default)]
    pub(crate) inverse_reduced_mobility: Option<f64>,
    /// Imaging pixel coordinates (imzML).
    #[serde(default)]
    pub(crate) position: Option<Vec<f64>>,
    /// The export's total ion current, when recorded.
    #[serde(default)]
    pub(crate) total_ion_current: Option<f64>,
    /// The export's base-peak m/z, when recorded.
    #[serde(default)]
    pub(crate) base_peak_mz: Option<f64>,
}
#[derive(Deserialize)]
pub(crate) struct OracleTdf {
    pub(crate) frames: Vec<OracleTdfFrame>,
    #[serde(default)]
    pub(crate) mz_samples: Vec<(u32, f64)>,
    #[serde(default)]
    pub(crate) mobility_samples: Vec<(u32, f64)>,
}
#[derive(Deserialize)]
pub(crate) struct OracleTdfFrame {
    pub(crate) id: i64,
    pub(crate) scans: usize,
    pub(crate) peaks: usize,
    #[serde(default)]
    pub(crate) xxh3_scan_offsets: Option<String>,
    #[serde(default)]
    pub(crate) xxh3_tof: Option<String>,
    #[serde(default)]
    pub(crate) xxh3_intensity: Option<String>,
    #[serde(default)]
    pub(crate) first_tof: Option<u32>,
    #[serde(default)]
    pub(crate) first_intensity: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct OracleImage {
    pub(crate) index: u32,
    #[serde(default)]
    pub(crate) size_x: u32,
    #[serde(default)]
    pub(crate) size_y: u32,
    #[serde(default)]
    pub(crate) size_z: u32,
    #[serde(default)]
    pub(crate) size_c: u32,
    #[serde(default)]
    pub(crate) size_t: u32,
    #[serde(default)]
    pub(crate) pixel_type: String,
    #[serde(default)]
    pub(crate) physical_size_um: BTreeMap<String, Option<f64>>,
    #[serde(default)]
    pub(crate) planes: Vec<OraclePlane>,
    /// The oracle could not read this image; only its presence is checked.
    #[serde(default)]
    pub(crate) skip: Option<String>,
    /// FLIM/TCSPC raw data: geometry is compared, reading a plane must be "unsupported".
    #[serde(default)]
    pub(crate) flim: bool,
    /// Stitched mosaic: per-tile offsets and (c0, z0, t0) tile hashes.
    #[serde(default)]
    pub(crate) mosaic: Option<OracleMosaic>,
    /// Downsampled pyramid levels (czifile `levels[1:]`).
    #[serde(default)]
    pub(crate) levels: Vec<OracleLevel>,
    /// Image name in our convention, compared when present (MetaMorph `.nd` stage labels).
    #[serde(default)]
    pub(crate) check_name: Option<String>,
    /// Channel names, compared when present (MetaMorph `.nd` wavelength names).
    #[serde(default)]
    pub(crate) check_channel_names: Option<Vec<String>>,
}
#[derive(Deserialize)]
pub(crate) struct OracleLevel {
    pub(crate) level: u32,
    pub(crate) size_x: u32,
    pub(crate) size_y: u32,
    #[serde(default)]
    pub(crate) planes: Vec<OraclePlane>,
}
#[derive(Deserialize)]
pub(crate) struct OracleMosaic {
    pub(crate) tile_width: u32,
    pub(crate) tile_height: u32,
    pub(crate) tiles: Vec<OracleTile>,
}
#[derive(Deserialize)]
pub(crate) struct OracleTile {
    pub(crate) index: u32,
    pub(crate) x_px: u32,
    pub(crate) y_px: u32,
    #[serde(default)]
    pub(crate) fully_visible: bool,
    #[serde(default)]
    pub(crate) xxh3_c0z0t0: Option<String>,
    /// `[x, y, w, h]` of the tile (tile coordinates) that no later tile covers, and its hash.
    #[serde(default)]
    pub(crate) visible: Option<[u32; 4]>,
    #[serde(default)]
    pub(crate) xxh3_visible: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct OraclePlane {
    pub(crate) c: u32,
    pub(crate) z: u32,
    pub(crate) t: u32,
    pub(crate) xxh3: String,
    /// Mean of all samples (written for lossy-compressed TIFF planes).
    #[serde(default)]
    pub(crate) mean: Option<f64>,
}
