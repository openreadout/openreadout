//! Plate-reader assay analysis: the standard next steps after reading a plate, as
//! deterministic, documented building blocks.
//!
//! - **Layouts**: well roles (blank, standard, sample, positive/negative control, empty),
//!   samples, compounds, concentrations and dilutions from a plate-map grid or long CSV, from
//!   the layout a vendor export embeds (Gen5 `Well ID`, SkanIt `Sample`, BMG `Content`), or from
//!   well flags.
//! - **Wells**: blank subtraction (mean of the blanks), replicate groups (mean, SD, CV %),
//!   outlier flags (modified z-score or Grubbs).
//! - **Standard curves**: linear, 4PL, 5PL least squares with weighting, parameter standard
//!   errors and confidence intervals, R², residuals, back-calculated concentrations with range
//!   flags and LLOQ/ULOQ.
//! - **Dose-response**: IC50/EC50 with confidence interval, Hill slope, top and bottom per
//!   compound, optionally normalised to controls.
//! - **Kinetics** and **growth curves**: max slope (Vmax), lag, time to max, AUC; growth rate,
//!   doubling time, logistic fit.
//! - **Assay quality**: Z′, signal/background, signal/noise, SSMD, CVs.
//!
//! Methods and conventions: `book/src/guides/plate-analysis.md`. Validation against SciPy, R `drc` and
//! `growthcurver` (run as black boxes) and against vendor-computed results stored in Gen5 and
//! SkanIt exports: `docs/provenance/plate-analysis.md`.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_assay::{Analysis, AssayRequest, analyze_file};
//! use openreadout_core::Registry;
//!
//! # fn registry() -> Registry { Registry::new() }
//! let req = AssayRequest {
//!     analysis: Analysis::Curve,
//!     standards: Some("A1,A2=100;B1,B2=50;C1,C2=25;D1,D2=12.5;E1,E2=6.25".into()),
//!     blank_wells: Some("H1,H2".into()),
//!     ..AssayRequest::default()
//! };
//! let out = analyze_file(&registry(), Path::new("elisa.txt"), &req)?;
//! for w in &out.wells {
//!     println!("{} {:?} {:?}", w.well, w.back_calculated, w.flag);
//! }
//! # Ok::<(), openreadout_core::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::many_single_char_names)]
// Exact float equality is intended where it is used: concentration levels, asymptotes and time
// stamps copied from the same source are compared as written.
#![allow(clippy::float_cmp)]

mod assay;
mod data;
pub mod fit;
pub mod kinetics;
pub mod layout;
mod linalg;
mod output;
mod plot;
mod request;
mod roles;
pub mod stats;
mod tables;

use std::path::Path;

use openreadout_core::{Error, Registry, Result};

pub use assay::{
    DEFAULT_GRUBBS_ALPHA, DEFAULT_MAD_THRESHOLD, DEFAULT_WINDOW, LOQ_TOLERANCE_PERCENT,
    MAD_MIN_GROUP, run, user_layout,
};
pub use data::{Obs, PlateData, ReadInfo};
pub use output::{
    AssayOutput, BlankSummary, CompoundRow, CurvePoint, CurveReport, FitReport, GroupStats,
    GrowthRow, KineticRow, LayoutSummary, OutlierSummary, ParamRow, QualityReport, ReadSummary,
    SampleRow, WellRow,
};
pub use plot::render as render_plot;
pub use request::{Analysis, AssayRequest, BlankMode, FitOn, Normalize, OutlierRule, Reduce};
pub use tables::{render_text, to_csv, write_tables, write_verified};

/// Largest long-CSV input read (plate data is small).
const MAX_CSV_BYTES: u64 = 64 << 20;

/// Load the plate of `path`: a long CSV (`well`, `value`, …) is read directly; anything else is
/// opened with the registry (plate-reader exports).
pub fn load_plate(reg: &Registry, path: &Path, table: u32) -> Result<PlateData> {
    let is_text = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "csv" | "tsv" | "txt" | "tab"
        )
    });
    if is_text {
        let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.len() <= MAX_CSV_BYTES {
            let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
            let text = String::from_utf8_lossy(&bytes);
            if PlateData::is_long_csv(&text) {
                return PlateData::from_long_csv(&text, &path.display().to_string());
            }
        }
    }
    let (_, mut ds) = reg.open(path)?;
    PlateData::from_dataset(ds.as_mut(), table)
}

/// Run `req` on the plate in `path` (see [`load_plate`]).
pub fn analyze_file(reg: &Registry, path: &Path, req: &AssayRequest) -> Result<AssayOutput> {
    let plate = load_plate(reg, path, req.table)?;
    let user = user_layout(req)?;
    let mut out = run(&plate, req, user)?;
    out.path = path.display().to_string();
    Ok(out)
}
