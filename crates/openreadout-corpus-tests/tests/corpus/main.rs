//! For every corpus file that is present on disk and has an oracle JSON, open it with our
//! reader and compare what the oracle holds: images (geometry, pixel type, physical sizes,
//! per-plane hashes), spectra and chromatograms, traces, tables and plates.
//!
//! Layout: this file walks the manifest and dispatches (`check_one`); `oracle` is the ground
//! truth's JSON shape; `images`, `spectra`, `traces` and `tables` compare one kind of output;
//! the `*_oracle` modules (and `hcs`) compare format families whose ground truth has a shape of
//! its own.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>[,<substring>...]` filters ids;
//! `CORPUS_FORMAT=<id>[,<id>...]` filters manifest formats;
//! `CORPUS_REPORT=path` writes a Markdown table.
#![cfg(feature = "corpus")]

#[path = "../support/registry.rs"]
mod shared_registry;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use serde::Deserialize;

use crate::images::{ImageTally, check_nd2_meta, compare_images};
use crate::oracle::Oracle;
use crate::spectra::start_time_agrees;
use crate::spectra::{check_chromatogram_traces, check_chromatograms, check_spectra, check_tdf};
use crate::tables::{check_plate, check_tables};
use crate::traces::check_traces;

mod biacore_oracle;
mod chromeleon_oracle;
mod fplc_oracle;
mod gpr_oracle;
mod hcs;
mod hyperspec_oracle;
mod images;
mod itc_oracle;
mod jasco_oracle;
mod oracle;
#[path = "../qpcr_oracle/mod.rs"]
mod qpcr_oracle;
mod seahorse_oracle;
mod series_oracle;
mod shimadzu_oracle;
mod spectra;
mod spike2x_oracle;
mod tables;
mod traces;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize, Clone)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    /// When set, the file is opened but not compared; the text says why the oracle disagrees.
    #[serde(default)]
    oracle_skip: Option<String>,
    /// Lossy codecs: accept planes whose samples differ from the oracle's decoded planes
    /// (`<files>/<stem>.oracle/image<i>_c<c>_z<z>_t<t>.bin`) by at most this much.
    #[serde(default)]
    pixel_tolerance: Option<u32>,
    /// Lossy codec (JPEG) in a downloaded file without oracle sidecars: a plane whose hash
    /// differs still passes when its mean is within `LOSSY_MEAN_TOLERANCE` of the oracle's.
    #[serde(default)]
    lossy: bool,
    /// Plate readers: compare value and well counts per mode but not the value hash; the text says
    /// why the oracle's well positions differ.
    #[serde(default)]
    plate_counts_only: Option<String>,
    /// Spectra: relative tolerance for precursor m/z when the export reports a value derived
    /// from several scans (Agilent MassHunter exports average the precursors of the scans that
    /// share one); the default is 1e-6.
    #[serde(default)]
    precursor_tolerance: Option<f64>,
    /// Spectra (Thermo): the export's converter took the monoisotopic m/z as the precursor only
    /// within this distance of the isolation target (ProteoWizard 3.0.4337: 1.5), where ours
    /// takes it within 3.0, as current converters do.
    #[serde(default)]
    export_monoisotopic_max_shift: Option<f64>,
    /// SRM chromatograms: absolute m/z tolerance between the export's product (Q3) target and
    /// the product m/z the scans record (added to the export's isolation offsets; default 1e-4).
    #[serde(default)]
    srm_product_tolerance: Option<f64>,
    /// Spectra: the export holds peaks the vendor library computed (peak picking) that the
    /// reader does not reproduce; the text says why. Metadata, total ion current and base-peak
    /// m/z are compared instead of the peak lists.
    #[serde(default)]
    peaks_not_compared: Option<String>,
    /// Spectra: the oracle converts m/z (and 1/K0) differently from the reader (timsrust's
    /// acquisition-range approximation where the reader applies the file's calibration, which
    /// `mz_agreement.rs` validates against vendor conversions); the text says why. Point counts
    /// and intensities are still compared, m/z values and 1/K0 are not.
    #[serde(default)]
    mz_not_compared: Option<String>,
    /// Spectra: the export names its spectra with ids of its own (Agilent ion-mobility data:
    /// frame × drift bin) where the reader keeps the file's scan ids; the text says why.
    #[serde(default)]
    native_id_not_compared: Option<String>,
    /// Spectra: the export reports a precursor and activation the file does not hold (all-ions
    /// frames: the scan window's centre as a pseudo-precursor); the text says why.
    #[serde(default)]
    precursor_not_compared: Option<String>,
    /// Spectra: the export calls a full scan with a collision energy MS2 and names the centre of
    /// its scan window as the precursor, where the file's record says MS level 1; such scans are
    /// compared without MS level, precursor and activation. The text says why.
    #[serde(default)]
    window_centre_precursor: Option<String>,
}

const LOSSY_MEAN_TOLERANCE: f64 = 0.05;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Every reader, in the CLI's detection order (`support/registry.rs`, kept equal to
/// `crates/openreadout-cli/src/registry.rs` by `openreadout`'s `tests/registry_parity.rs`).
fn registry() -> Registry {
    shared_registry::registry()
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1e-12)
}

fn xxh3_f64(v: &[f64]) -> String {
    let mut bytes = Vec::with_capacity(v.len() * 8);
    for x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes))
}

fn xxh3_f32(v: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes))
}

thread_local! {
    /// Normalized fields (`openreadout_core::assurance::TRACKED_FIELDS` paths) the comparison of
    /// the current file checked against its oracle; `Outcome::fields` takes them.
    static COVERED: std::cell::RefCell<std::collections::BTreeSet<&'static str>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

/// The comparison checked `field` (a tracked field path) against the oracle.
fn covered(field: &'static str) {
    COVERED.with(|c| {
        c.borrow_mut().insert(field);
    });
}

/// The fields covered since the last call.
fn take_covered() -> Vec<&'static str> {
    COVERED.with(|c| std::mem::take(&mut *c.borrow_mut()).into_iter().collect())
}

struct Outcome {
    id: String,
    format: String,
    status: String,
    detail: String,
    /// The oracle is an independent reader (not a second implementation of our own notes).
    independent: bool,
    /// Which outputs the comparison covered (`metadata`, `pixels`, `spectra`, `traces`,
    /// `tables`): the assurance audit counts a feature as validated only on the outputs it affects.
    compared: Vec<&'static str>,
    /// Tracked normalized fields the comparison checked (`covered`): the audit validates a field
    /// or a rule deriving it only on files whose comparison checked that field.
    fields: Vec<&'static str>,
}

/// The outputs an oracle lets the comparison check (`docs/assurance.md`). A Thermo file's
/// chromatograms are rebuilt from its spectra (`check_chromatograms`: the TIC from every scan, an
/// SRM trace from the peaks in each scan's product window), so they check its spectra; its traces
/// are the LC detectors', which no chromatogram covers. In the other vendor formats an SRM
/// chromatogram is rebuilt from the spectra too, so it checks the spectra; their TIC and BPC
/// can come from our traces, so those check traces.
fn compared_scopes(o: &Oracle, format: &str) -> Vec<&'static str> {
    let thermo_chromatograms = format == "thermo-raw" && o.chromatograms.is_some();
    let srm_from_spectra = !matches!(format, "mzml" | "mzxml")
        && o.chromatograms
            .as_ref()
            .is_some_and(|c| c.iter().any(|t| t.kind == "srm"));
    let mut v = vec!["metadata"];
    if o.images
        .iter()
        .any(|i| !i.planes.is_empty() || i.mosaic.is_some() || !i.levels.is_empty())
        || o.hcs.is_some()
    {
        v.push("pixels");
    }
    if o.spectra.is_some() || o.tdf.is_some() || thermo_chromatograms || srm_from_spectra {
        v.push("spectra");
    }
    if o.traces.iter().any(|t| t.sweep_count.is_some())
        || (o.chromatograms.is_some() && !thermo_chromatograms)
    {
        v.push("traces");
    }
    if o.tables.iter().any(|t| {
        t.xxh3.is_some()
            || !t.column_hashes.is_empty()
            || !t.sorted_column_hashes.is_empty()
            || !t.sorted_row_hashes.is_empty()
    }) || o.plate.is_some()
    {
        v.push("tables");
    }
    v
}

/// What a family comparison covers: a Shimadzu oracle compares vendor peak tables
/// only when a LabSolutions export holds them, and spectra only with LC-MS ground truth.
fn family_scopes(format: &str, oracle_text: &str) -> Vec<&'static str> {
    if format == "shimadzu" {
        let o: serde_json::Value = serde_json::from_str(oracle_text).unwrap_or_default();
        let mut v = vec!["metadata", "traces"];
        if o["shimadzu_export"]["peak_tables"]
            .as_object()
            .is_some_and(|t| !t.is_empty())
        {
            v.push("tables");
        }
        if o["ms_ground_truth"]["spectra"]
            .as_array()
            .is_some_and(|s| !s.is_empty())
        {
            v.push("spectra");
        }
        v
    } else if format == "chromeleon" {
        // a PDF report with integration results compares the vendor_peaks table too
        let o: serde_json::Value = serde_json::from_str(oracle_text).unwrap_or_default();
        let mut v = vec!["metadata", "traces"];
        if o["pdf_report"]["pages"].as_array().is_some_and(|p| {
            p.iter()
                .any(|p| p["peaks"].as_array().is_some_and(|a| !a.is_empty()))
        }) {
            v.push("tables");
        }
        v
    } else if format == "jasco-jws" {
        vec!["metadata", "traces"]
    } else if format == "genepix-gpr" {
        vec!["metadata", "tables"]
    } else if series_oracle::FORMATS.contains(&format) {
        series_oracle::scopes(format)
    } else if hyperspec_oracle::FORMATS.contains(&format) {
        hyperspec_oracle::scopes(oracle_text)
    } else {
        vec!["metadata", "tables", "traces"]
    }
}

/// Compare one corpus file with its oracle: the outputs the oracle holds (plate, HCS layout,
/// timsTOF frames, spectra, chromatograms, images, traces, tables, ND2 metadata, frame
/// records).
fn check_one(
    reg: &Registry,
    path: &Path,
    oracle: &Oracle,
    tolerance: Option<u32>,
    lossy: bool,
    plate_counts_only: bool,
) -> Result<String, String> {
    let (_, mut ds) = reg.open(path).map_err(|e| format!("open failed: {e}"))?;
    if oracle.spectra.is_some()
        && oracle.export_excludes_flagged_peaks()
        && ds
            .info()
            .is_ok_and(|i| i.format.id == openreadout_thermo::FORMAT_ID)
    {
        // Compare with what that converter wrote: flagged reference peaks left out.
        let mut t = openreadout_thermo::dataset::ThermoDataset::open(path)
            .map_err(|e| format!("open failed: {e}"))?;
        t.set_exclude_flagged_peaks(true);
        ds = Box::new(t);
    }
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    if let Some(p) = &oracle.plate {
        return check_plate(ds.as_mut(), &info, p, plate_counts_only);
    }
    if let Some(h) = &oracle.hcs {
        return hcs::check_hcs(ds.as_mut(), &info, h);
    }
    let tdf = oracle.tdf.as_ref().map(|t| check_tdf(path, t));
    if let Some(t) = tdf {
        let spectra = match &oracle.spectra {
            Some(sp) => check_spectra(ds.as_mut(), &info, sp),
            None => Ok(String::new()),
        };
        return match (t, spectra) {
            (Ok(a), Ok(b)) if b.is_empty() => Ok(a),
            (Ok(a), Ok(b)) => Ok(format!("{a}; {b}")),
            (Err(a), Ok(_)) => Err(a),
            (Ok(_), Err(b)) => Err(b),
            (Err(a), Err(b)) => Err(format!("{a}; {b}")),
        };
    }
    if let Some(sp) = &oracle.spectra
        && oracle.tables.is_empty()
    {
        let spectra = check_spectra(ds.as_mut(), &info, sp);
        if oracle.traces.is_empty() {
            return spectra;
        }
        let mut problems = Vec::new();
        let mut ok_sweeps = 0usize;
        check_traces(ds.as_mut(), &info, oracle, &mut problems, &mut ok_sweeps);
        return match spectra {
            Ok(s) if problems.is_empty() => Ok(format!(
                "{s}; {} chromatograms, {ok_sweeps} arrays match",
                oracle.traces.len()
            )),
            Ok(s) => Err(format!("{s}; chromatograms: {}", problems.join("; "))),
            Err(e) => Err(if problems.is_empty() {
                e
            } else {
                format!("{e}; chromatograms: {}", problems.join("; "))
            }),
        };
    }
    // Chromatography data sets can hold spectra and traces (a ChemStation .D with .ch and .ms
    // files, an ANDI/MS file with its total-ion chromatogram): compare both.
    let spectra_result = oracle
        .spectra
        .as_ref()
        .map(|sp| check_spectra(ds.as_mut(), &info, sp));
    if let Some(r) = spectra_result.clone()
        && oracle.traces.is_empty()
        && oracle.tables.is_empty()
    {
        return r;
    }
    if let Some(tr) = &oracle.chromatograms {
        if matches!(info.format.id.as_str(), "mzml" | "mzxml") {
            return check_chromatogram_traces(ds.as_mut(), &info, tr);
        }
        return check_chromatograms(ds.as_mut(), &info, tr, oracle.srm_product_tolerance);
    }
    let mut problems = Vec::new();
    let ImageTally {
        ok_planes,
        tol_planes,
        lossy_planes,
        worst,
        level_planes,
        ok_tiles,
        flim_ok,
        names_ok,
    } = compare_images(
        ds.as_mut(),
        &info,
        oracle,
        path,
        tolerance,
        lossy,
        &mut problems,
    );
    let mut ok_sweeps = 0usize;
    if !oracle.traces.is_empty() {
        check_traces(ds.as_mut(), &info, oracle, &mut problems, &mut ok_sweeps);
    }
    let mut ok_tables = 0usize;
    let mut unhashed = 0usize;
    if !oracle.tables.is_empty() {
        check_tables(
            ds.as_mut(),
            &info,
            oracle,
            &mut problems,
            &mut ok_tables,
            &mut unhashed,
        );
    }
    let mut meta_note = String::new();
    if ok_tiles > 0 {
        let _ = write!(meta_note, ", {ok_tiles} tile placements verified");
    }
    if flim_ok > 0 {
        let _ = write!(meta_note, ", {flim_ok} FLIM images unsupported as expected");
    }
    if let Some(m) = &oracle.nd2_meta {
        let (checked, mut p) = check_nd2_meta(ds.as_ref(), &info, m, oracle.is_legacy);
        problems.append(&mut p);
        let _ = write!(meta_note, ", {checked} metadata values match");
    }
    if names_ok > 0 {
        let _ = write!(meta_note, ", {names_ok} image/channel names match");
    }
    if let Some(frames) = &oracle.check_frames {
        match ds.frames(0, None) {
            Ok((_, recs)) => {
                let mut ok = 0usize;
                for f in frames {
                    let r = recs.iter().find(|r| {
                        r.get("t").and_then(serde_json::Value::as_u64) == Some(u64::from(f.t))
                    });
                    let frame = r
                        .and_then(|r| r.get("frame"))
                        .and_then(serde_json::Value::as_u64);
                    let at = r
                        .and_then(|r| r.get("acquired_at"))
                        .and_then(serde_json::Value::as_str);
                    if f.frame.is_none_or(|v| frame == Some(v))
                        && f.acquired_at.as_deref().is_none_or(|v| at == Some(v))
                    {
                        ok += 1;
                    } else {
                        problems.push(format!(
                            "frame record t={}: frame {frame:?} / {at:?} != oracle {:?} / {:?}",
                            f.t, f.frame, f.acquired_at
                        ));
                    }
                }
                let _ = write!(meta_note, ", {ok} frame records match");
            }
            Err(e) => problems.push(format!("frames: {e}")),
        }
    }
    let result = if problems.is_empty() {
        if !oracle.traces.is_empty() {
            let tables = if oracle.tables.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} tables, {ok_tables} event matrices match",
                    oracle.tables.len()
                )
            };
            // files with traces and images (a Raman map: spectra and the map image)
            let images = if oracle.images.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} images, {ok_planes} planes match{}",
                    oracle.images.len(),
                    if lossy_planes > 0 {
                        format!(", {lossy_planes} lossy planes within tolerance")
                    } else {
                        String::new()
                    }
                )
            };
            Ok(format!(
                "{} traces, {ok_sweeps} sweep x channel blocks match (sample count, first samples, xxh3){tables}{images}",
                oracle.traces.len()
            ))
        } else if oracle.tables.is_empty() {
            let mut s = format!(
                "{} images, {ok_planes} planes match{meta_note}",
                oracle.images.len()
            );
            if tol_planes > 0 {
                s.push_str(&format!(
                    ", {tol_planes} within tolerance (max |diff| {worst})"
                ));
            }
            if lossy_planes > 0 {
                s.push_str(&format!(", {lossy_planes} lossy planes within tolerance"));
            }
            if level_planes > 0 {
                s.push_str(&format!(", {level_planes} pyramid-level planes match"));
            }
            Ok(s)
        } else {
            Ok(format!(
                "{} tables, {ok_tables} event matrices match{}",
                oracle.tables.len(),
                if unhashed > 0 {
                    format!(" ({unhashed} without an oracle hash: metadata only)")
                } else {
                    String::new()
                }
            ))
        }
    } else {
        Err(format!(
            "{ok_planes} planes ok, {ok_tables} tables ok, {ok_sweeps} sweep blocks ok; {}",
            problems.join("; ")
        ))
    };
    match (spectra_result, result) {
        (None, r) => r,
        (Some(Ok(a)), Ok(b)) => Ok(format!("{a}; {b}")),
        (Some(Err(a)), Ok(b) | Err(b)) | (Some(Ok(b)), Err(a)) => Err(format!("{a}; {b}")),
    }
}

/// Files with `role = "corrupt"` must be rejected: `open` fails with a corrupt-file error, or
/// `check` reports at least one error.
fn check_corrupt(reg: &Registry, path: &Path) -> Result<String, String> {
    match reg.open(path) {
        Err(e @ openreadout_core::Error::Corrupt { .. }) => {
            Ok(format!("rejected on open (exit {}): {e}", e.exit_code()))
        }
        Err(e) => Err(format!("open failed but not as corrupt: {e}")),
        Ok((_, mut ds)) => match ds.check() {
            Ok(r) if !r.ok => Ok(format!(
                "check reports {}",
                r.findings
                    .iter()
                    .filter(|f| f.severity == openreadout_core::model::Severity::Error)
                    .map(|f| f.code.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Ok(_) => Err("check reports no errors for a known-corrupt file".into()),
            Err(e) => Err(format!("check failed: {e}")),
        },
    }
}

/// Formats whose ground truth is not the generic `Oracle` shape: qPCR (`oracle/qpcr.py`) and
/// Shimadzu LabSolutions (chromConverter, or the vendor's ASCII export), Chromeleon (the vendor's
/// ASCII exports or PDF report, `oracle/chromeleon_oracle.py`). Development oracles live
/// in `corpus/oracle/<family>/<id>.json`, held-out ones in `corpus/oracle/heldout/<id>.json`;
/// both go through the same comparison here. `None` for every other format.
fn family_dir(format: &str) -> Option<&'static str> {
    if qpcr_oracle::FORMATS.contains(&format) {
        Some("qpcr")
    } else if format == "shimadzu" {
        Some("shimadzu")
    } else if format == "chromeleon" {
        Some("chromeleon")
    } else if fplc_oracle::FORMATS.contains(&format) {
        Some("fplc")
    } else if itc_oracle::FORMATS.contains(&format) {
        Some("itc")
    } else if biacore_oracle::FORMATS.contains(&format) {
        Some("biacore")
    } else if jasco_oracle::FORMATS.contains(&format) {
        Some("jasco")
    } else if seahorse_oracle::FORMATS.contains(&format) {
        Some("seahorse")
    } else if gpr_oracle::FORMATS.contains(&format) {
        Some("gpr")
    } else if series_oracle::FORMATS.contains(&format) {
        Some("series")
    } else if hyperspec_oracle::FORMATS.contains(&format) {
        Some("hyperspec")
    } else {
        None
    }
}

/// `family_dir`, or `spike2x` for a Spike2 entry whose oracle comes from the vendor's own export
/// (`corpus/oracle/spike2x/<id>.json`: the 64-bit `.smrx` files).
fn family_for(format: &str, id: &str) -> Option<&'static str> {
    family_dir(format).or_else(|| {
        (format == "ced-spike2"
            && root()
                .join("corpus/oracle")
                .join(spike2x_oracle::DIR)
                .join(format!("{id}.json"))
                .exists())
        .then_some(spike2x_oracle::DIR)
    })
}

/// Keys of a family oracle that only identify the file (they hold no ground truth).
const IDENTITY_KEYS: &[&str] = &[
    "id",
    "format",
    "file",
    "size",
    "sha256",
    "independent",
    "source",
    "note",
    "notes",
    "reader",
    "tool",
];

/// True when a family oracle holds nothing to compare: every key identifies the file, or its
/// value is empty. A comparison against it checks only the file's own consistency, so it is
/// reported `no-oracle`, never `pass` (held-out draw C, finding C-O3).
fn family_oracle_is_empty(text: &str) -> bool {
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    if o.contains_key("error") {
        return false;
    }
    !o.iter().any(|(k, v)| {
        !IDENTITY_KEYS.contains(&k.as_str())
            && match v {
                serde_json::Value::Null => false,
                serde_json::Value::Array(a) => !a.is_empty(),
                serde_json::Value::Object(m) => !m.is_empty(),
                serde_json::Value::String(t) => !t.is_empty(),
                _ => true,
            }
    })
}

/// Compare a qPCR, Shimadzu or Chromeleon entry with its oracle text; `None` for other formats.
fn check_family(format: &str, id: &str, path: &Path, text: &str) -> Option<Result<String, String>> {
    let family = family_for(format, id)?;
    let o: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("oracle JSON: {e}"))),
    };
    if let Some(err) = o["error"].as_str() {
        return Some(Err(format!("oracle error: {err}")));
    }
    Some(if family == spike2x_oracle::DIR {
        spike2x_oracle::compare_file(id, path, &o)
    } else if format == "shimadzu" {
        shimadzu_oracle::compare_file(id, path, &o)
    } else if format == "chromeleon" {
        chromeleon_oracle::compare_file(id, path, &o)
    } else if fplc_oracle::FORMATS.contains(&format) {
        fplc_oracle::compare_file(format, id, path, &o)
    } else if itc_oracle::FORMATS.contains(&format) {
        itc_oracle::compare_file(id, path, &o)
    } else if biacore_oracle::FORMATS.contains(&format) {
        biacore_oracle::compare_file(id, path, &o)
    } else if jasco_oracle::FORMATS.contains(&format) {
        jasco_oracle::compare_file(id, path, &o)
    } else if seahorse_oracle::FORMATS.contains(&format) {
        seahorse_oracle::compare_file(id, path, &o)
    } else if gpr_oracle::FORMATS.contains(&format) {
        gpr_oracle::compare_file(id, path, &o)
    } else if series_oracle::FORMATS.contains(&format) {
        series_oracle::compare_file(format, id, path, &o)
    } else if hyperspec_oracle::FORMATS.contains(&format) {
        hyperspec_oracle::compare_file(format, id, path, &o)
    } else {
        qpcr_oracle::compare_file(format, id, path, &o)
    })
}

/// The comparison settings a manifest entry sets on its oracle (tolerances, values not compared).
/// An mzML/mzXML file is compared with pyteomics' reading of that same file, spectrum by spectrum
/// in file order (a Thermo RAW entry sharing the id maps by scan number instead).
fn apply_entry_settings(oracle: &mut Oracle, e: &Entry) {
    if (e.format == "mzml" || e.format == "mzxml" || e.format == "imzml")
        && let Some(sp) = oracle.spectra.as_mut()
    {
        sp.by_index = true;
    }
    if let Some(sp) = oracle.spectra.as_mut() {
        sp.precursor_tolerance = e.precursor_tolerance;
        sp.export_monoisotopic_max_shift = e.export_monoisotopic_max_shift;
        sp.peaks_not_compared.clone_from(&e.peaks_not_compared);
        sp.mz_not_compared.clone_from(&e.mz_not_compared);
        sp.native_id_not_compared
            .clone_from(&e.native_id_not_compared);
        sp.precursor_not_compared
            .clone_from(&e.precursor_not_compared);
        sp.window_centre_precursor
            .clone_from(&e.window_centre_precursor);
    }
    oracle.srm_product_tolerance = e.srm_product_tolerance;
}

/// `CORPUS_ONLY` holds one id substring or several separated by commas.
fn only_matches(only: &str, id: &str) -> bool {
    only.split(',').any(|o| id.contains(o.trim()))
}

#[test]
fn corpus_matches_oracle() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let formats = std::env::var("CORPUS_FORMAT").ok();
    let reg = registry();
    let mut outcomes = Vec::new();
    for e in manifest.file.iter().filter(|e| {
        e.role.is_empty()
                || e.role == "input"
                || e.role == "corrupt"
                // depositor mzML/mzXML exports are open-format inputs in their own right
                || (e.role == "oracle-export" && (e.format == "mzml" || e.format == "mzxml"))
    }) {
        if only.as_ref().is_some_and(|o| !only_matches(o, &e.id)) {
            continue;
        }
        if formats
            .as_ref()
            .is_some_and(|f| !f.split(',').any(|x| x.trim() == e.format))
        {
            continue;
        }
        let _ = take_covered();
        if e.role == "corrupt" {
            let path = files_dir.join(&e.filename);
            if !path.exists() {
                continue;
            }
            let outcome = match check_corrupt(&reg, &path) {
                Ok(d) => Outcome {
                    id: e.id.clone(),
                    format: e.format.clone(),
                    status: "pass".into(),
                    detail: d,
                    independent: false,
                    compared: Vec::new(),
                    fields: Vec::new(),
                },
                Err(d) => Outcome {
                    id: e.id.clone(),
                    format: e.format.clone(),
                    status: "FAIL".into(),
                    detail: d,
                    independent: false,
                    compared: Vec::new(),
                    fields: Vec::new(),
                },
            };
            println!(
                "{:<6} {:<5} {:<60} {}",
                outcome.status, outcome.format, outcome.id, outcome.detail
            );
            outcomes.push(outcome);
            continue;
        }
        let path = files_dir.join(&e.filename);
        // `<id>.json` (electrophysiology files that share a stem) or `<stem>.json`; qPCR and
        // Shimadzu: `<family>/<id>.json`.
        let by_id = root.join("corpus/oracle").join(format!("{}.json", e.id));
        let oracle_path = if let Some(dir) = family_for(&e.format, &e.id) {
            root.join("corpus/oracle")
                .join(dir)
                .join(format!("{}.json", e.id))
        } else if openreadout_corpus_tests::oracle_json::exists(&by_id) {
            by_id
        } else {
            root.join("corpus/oracle").join(format!(
                "{}.json",
                e.filename
                    .rsplit_once('.')
                    .map_or(e.filename.as_str(), |(s, _)| s)
            ))
        };
        // over 1 MiB the oracle is stored as `<name>.json.gz`
        if !path.exists() || !openreadout_corpus_tests::oracle_json::exists(&oracle_path) {
            continue;
        }
        let text = openreadout_corpus_tests::oracle_json::read_to_string(&oracle_path).unwrap();
        if let Some(r) = check_family(&e.format, &e.id, &path, &text) {
            let (status, detail) = match r {
                Ok(d) if family_oracle_is_empty(&text) => (
                    "no-oracle",
                    format!("the oracle holds no values (self-consistency only: {d})"),
                ),
                Ok(d) => ("pass", d),
                Err(d) => ("FAIL", d),
            };
            // a family oracle that is a second implementation of our notes says so
            let independent = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("independent").and_then(serde_json::Value::as_bool))
                .unwrap_or(true);
            let outcome = Outcome {
                id: e.id.clone(),
                format: e.format.clone(),
                status: status.into(),
                detail,
                independent,
                compared: family_scopes(&e.format, &text),
                fields: take_covered(),
            };
            println!(
                "{:<6} {:<5} {:<60} {}",
                outcome.status, outcome.format, outcome.id, outcome.detail
            );
            outcomes.push(outcome);
            continue;
        }
        let mut oracle: Oracle = serde_json::from_str(&text).unwrap();
        apply_entry_settings(&mut oracle, e);
        let outcome = if let Some(why) = &e.oracle_skip {
            let status = match reg.open(&path).and_then(|(_, ds)| ds.info()) {
                Ok(_) => "skip".to_string(),
                Err(openreadout_core::Error::Unsupported { .. }) => "skip".to_string(),
                Err(err) => format!("FAIL (open: {err})"),
            };
            Outcome {
                id: e.id.clone(),
                format: e.format.clone(),
                status,
                detail: why.clone(),
                independent: oracle.independent,
                compared: Vec::new(),
                fields: Vec::new(),
            }
        } else if let Some(err) = &oracle.error {
            Outcome {
                id: e.id.clone(),
                format: e.format.clone(),
                status: "oracle-error".into(),
                detail: err.clone(),
                independent: oracle.independent,
                compared: Vec::new(),
                fields: Vec::new(),
            }
        } else {
            // Mass-spectrometry runs: our acquisition start against the depositor export's
            // `startTimeStamp` (a coverage check only: it earns coverage when they agree).
            if let Some(export) = manifest
                .file
                .iter()
                .find(|x| x.id == e.id && x.role == "oracle-export")
            {
                start_time_agrees(&reg, &path, &files_dir.join(&export.filename));
            }
            match check_one(
                &reg,
                &path,
                &oracle,
                e.pixel_tolerance,
                e.lossy,
                e.plate_counts_only.is_some(),
            ) {
                Ok(d) => Outcome {
                    id: e.id.clone(),
                    format: e.format.clone(),
                    status: "pass".into(),
                    detail: if oracle.independent {
                        d
                    } else {
                        format!("{d} [self-consistency: no independent reader exists]")
                    },
                    independent: oracle.independent,
                    compared: compared_scopes(&oracle, &e.format),
                    fields: take_covered(),
                },
                Err(d) => Outcome {
                    id: e.id.clone(),
                    format: e.format.clone(),
                    status: "FAIL".into(),
                    detail: d,
                    independent: oracle.independent,
                    compared: compared_scopes(&oracle, &e.format),
                    fields: take_covered(),
                },
            }
        };
        println!(
            "{:<6} {:<5} {:<60} {}",
            outcome.status, outcome.format, outcome.id, outcome.detail
        );
        outcomes.push(outcome);
    }
    if let Ok(p) = std::env::var("CORPUS_REPORT") {
        let mut md = String::from("| status | format | id | detail |\n| --- | --- | --- | --- |\n");
        for o in &outcomes {
            let _ = writeln!(
                md,
                "| {} | {} | `{}` | {} |",
                o.status,
                o.format,
                o.id,
                o.detail.replace('|', "\\|")
            );
        }
        std::fs::write(p, md).unwrap();
    }
    // Machine-readable results for `cargo xtask assurance-audit refresh` (docs/assurance.md).
    if let Ok(p) = std::env::var("CORPUS_RESULTS") {
        let mut lines = String::new();
        for o in &outcomes {
            let _ = writeln!(
                lines,
                "{}",
                serde_json::json!({"id": o.id, "format": o.format, "status": o.status, "independent": o.independent, "compared": o.compared, "fields": o.fields})
            );
        }
        std::fs::write(p, lines).unwrap();
    }
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| o.status == "FAIL").collect();
    assert!(
        outcomes.iter().any(|o| o.status == "pass"),
        "no corpus files were checked; run `cargo xtask corpus fetch --tier smoke` and oracle/gen.py"
    );
    assert!(
        failed.is_empty(),
        "{} corpus files failed:\n{}",
        failed.len(),
        failed
            .iter()
            .map(|o| format!("  {} — {}", o.id, o.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The held-out set (`role = "heldout"`, docs/benchmark/heldout.md): the same comparison against
/// `corpus/oracle/heldout/<id>.json`, but it only RECORDS agreement and never fails: held-out files
/// measure generalization and are never used to fix a reader (a fix is developed on a new file).
/// A reader panic is caught and recorded as `PANIC`. `HELDOUT_REPORT=path` writes one JSON object
/// per file (id, format, status, detail) for the generalization report.
#[test]
fn heldout_agreement_is_recorded() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let mut lines = String::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in manifest.file.iter().filter(|e| e.role == "heldout") {
        if only.as_ref().is_some_and(|o| !only_matches(o, &e.id)) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        let oracle_path = root
            .join("corpus/oracle/heldout")
            .join(format!("{}.json", e.id));
        let (status, detail) = if !path.exists() {
            (
                "absent".to_string(),
                "not downloaded (cargo xtask corpus fetch --tier heldout)".to_string(),
            )
        } else if !openreadout_corpus_tests::oracle_json::exists(&oracle_path) {
            ("no-oracle".to_string(), "no oracle JSON".to_string())
        } else {
            let text = openreadout_corpus_tests::oracle_json::read_to_string(&oracle_path).unwrap();
            let mut oracle: Option<Oracle> = None;
            if family_for(&e.format, &e.id).is_none() {
                let mut o: Oracle = serde_json::from_str(&text).unwrap();
                // the same comparison settings as a development file's
                apply_entry_settings(&mut o, e);
                oracle = Some(o);
            }
            if let Some(err) = oracle.as_ref().and_then(|o| o.error.clone()) {
                ("oracle-error".to_string(), err)
            } else {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match &oracle {
                        // qPCR and Shimadzu: the same comparison as their development files
                        None => check_family(&e.format, &e.id, &path, &text)
                            .unwrap_or_else(|| Err("no comparison for this format".into())),
                        Some(o) => check_one(
                            &reg,
                            &path,
                            o,
                            e.pixel_tolerance,
                            e.lossy,
                            e.plate_counts_only.is_some(),
                        ),
                    }
                }));
                match run {
                    Ok(Ok(d)) if oracle.is_none() && family_oracle_is_empty(&text) => (
                        "no-oracle".to_string(),
                        format!("the oracle holds no values (self-consistency only: {d})"),
                    ),
                    Ok(Ok(d)) => ("pass".to_string(), d),
                    Ok(Err(d)) => ("FAIL".to_string(), d),
                    Err(p) => (
                        "PANIC".to_string(),
                        p.downcast_ref::<String>()
                            .cloned()
                            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                            .unwrap_or_default(),
                    ),
                }
            }
        };
        println!("{status:<12} {:<12} {:<50} {detail}", e.format, e.id);
        *counts.entry(status.clone()).or_default() += 1;
        let _ = writeln!(
            lines,
            "{}",
            serde_json::json!({"id": e.id, "format": e.format, "status": status, "detail": detail})
        );
    }
    println!("held-out agreement (recorded, never asserted): {counts:?}");
    if let Ok(p) = std::env::var("HELDOUT_REPORT") {
        std::fs::write(p, lines).unwrap();
    }
}
