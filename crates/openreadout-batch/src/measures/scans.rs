//! `batch spectra`: the scan headers of mass-spectrometry runs (`spectra` over many files), one row
//! per data set × scan passing the filter, read without decoding peaks
//! (`openreadout_core::scans`).
//!
//! Options (the `openreadout_spectra` filters): `run`, `ms_level`, `polarity`, `rt_range`
//! ([start, end] minutes), `precursor`, `precursor_tol`, `precursor_ppm`, `charge`,
//! `activation`, `scan_filter`.

use openreadout_core::scans::{ScanFilter, visit_scans};
use openreadout_core::{Error, Result};
use serde::Deserialize;

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};

/// `spectra` options.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScansOptions {
    #[serde(default)]
    run: u32,
    ms_level: Option<u32>,
    polarity: Option<String>,
    rt_range: Option<[f64; 2]>,
    precursor: Option<f64>,
    precursor_tol: Option<f64>,
    precursor_ppm: Option<f64>,
    charge: Option<i32>,
    activation: Option<String>,
    scan_filter: Option<String>,
}

/// Scan headers as rows.
#[derive(Debug, Clone, Default)]
pub struct ScansMeasure {
    run: u32,
    filter: ScanFilter,
}

impl ScansMeasure {
    /// From the batch `options` (unknown names are a usage error listing the valid ones).
    pub fn from_options(options: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        let o: ScansOptions =
            serde_json::from_value(serde_json::Value::Object(options.clone())).map_err(|e| {
                Error::Usage(format!(
                    "`spectra` options: {e} (valid: run, ms_level, polarity, rt_range, precursor, precursor_tol, precursor_ppm, charge, activation, scan_filter)"
                ))
            })?;
        let filter = ScanFilter {
            ms_level: o.ms_level,
            polarity: o.polarity.map(|p| p.to_ascii_lowercase()),
            rt_min_s: o.rt_range.map(|r| r[0] * 60.0),
            rt_max_s: o.rt_range.map(|r| r[1] * 60.0),
            precursor_mz: o.precursor,
            precursor_tol_mz: o.precursor_tol,
            precursor_tol_ppm: o.precursor_ppm,
            charge: o.charge,
            activation: o.activation,
            filter_contains: o.scan_filter,
        };
        filter.validate()?;
        Ok(ScansMeasure { run: o.run, filter })
    }
}

impl Measure for ScansMeasure {
    fn id(&self) -> &'static str {
        "spectra"
    }
    fn grain(&self) -> Vec<String> {
        vec!["run".into(), "index".into()]
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        vec![
            ColumnDoc::key("run", "run index, from 0"),
            ColumnDoc::key("index", "spectrum index within the run, from 0"),
            ColumnDoc::value("scan_number", "scan number as the instrument counts it"),
            ColumnDoc::value("native_id", "the spectrum's identifier in its file"),
            ColumnDoc::value("ms_level", "MS level (1 = full scan, 2 = MS/MS)"),
            ColumnDoc::unit("rt_min", "min", "retention time"),
            ColumnDoc::value("polarity", "positive, negative or unknown"),
            ColumnDoc::value("centroided", "the stored peaks are centroids"),
            ColumnDoc::unit("precursor_mz", "m/z", "isolated precursor"),
            ColumnDoc::value("precursor_charge", "precursor charge state"),
            ColumnDoc::unit("isolation_lower_mz", "m/z", "isolation window, lower edge"),
            ColumnDoc::unit("isolation_upper_mz", "m/z", "isolation window, upper edge"),
            ColumnDoc::value("activation", "CID, HCD, ETD, …"),
            ColumnDoc::value("collision_energy", "collision energy as recorded"),
            ColumnDoc::value("scan_filter", "instrument filter or scan description"),
            ColumnDoc::value(
                "total_ion_current",
                "TIC as the file stores it for the scan",
            ),
            ColumnDoc::unit("base_peak_mz", "m/z", "base peak as the file stores it"),
            ColumnDoc::value("point_count", "stored points, when the header records it"),
        ]
    }
    fn headers_only(&self) -> bool {
        true
    }
    fn fingerprint(&self) -> String {
        format!(
            "scans:{}:{}",
            self.run,
            serde_json::to_string(&self.filter).unwrap_or_default()
        )
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        if it.info.spectra.is_empty() {
            return Err(super::not_applicable(
                "spectra",
                it.info,
                "mass spectra",
                "Use `chromatogram` or `trace` for detector signals.",
            ));
        }
        let mut rows = Vec::new();
        visit_scans(it.dataset, self.run, &mut |h| {
            if !self.filter.matches(&h) {
                return true;
            }
            let mut r = Row::new().with("run", self.run).with("index", h.index);
            r.set("scan_number", h.scan_number);
            r.set("native_id", Value::text_opt(h.native_id.as_deref()));
            r.set("ms_level", h.ms_level);
            r.set("rt_min", Value::float_opt(h.rt_s.map(|t| t / 60.0)));
            r.set("polarity", h.polarity.as_str());
            r.set("centroided", h.centroided);
            r.set("precursor_mz", Value::float_opt(h.precursor_mz));
            r.set(
                "precursor_charge",
                h.precursor_charge
                    .map_or(Value::Null, |z| Value::from(i64::from(z))),
            );
            r.set(
                "isolation_lower_mz",
                Value::float_opt(h.isolation_window_mz.map(|w| w[0])),
            );
            r.set(
                "isolation_upper_mz",
                Value::float_opt(h.isolation_window_mz.map(|w| w[1])),
            );
            r.set("activation", Value::text_opt(h.activation.as_deref()));
            r.set("collision_energy", Value::float_opt(h.collision_energy));
            r.set("scan_filter", Value::text_opt(h.scan_filter.as_deref()));
            r.set("total_ion_current", Value::float_opt(h.total_ion_current));
            r.set("base_peak_mz", Value::float_opt(h.base_peak_mz));
            r.set(
                "point_count",
                h.point_count.map_or(Value::Null, Value::from),
            );
            rows.push(r);
            true
        })?;
        Ok(rows)
    }
}
