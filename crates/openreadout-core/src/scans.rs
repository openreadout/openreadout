//! Header-only listing of the spectra (scans) of a mass-spectrometry run: scan number, MS level,
//! retention time, polarity, precursor m/z and charge, isolation window, activation, filter
//! string, and the instrument's stored TIC/base peak where the file keeps them, without decoding
//! any peak arrays.
//!
//! Readers stream headers through [`Dataset::visit_scan_headers`]; readers without a
//! header-only path fall back to decoding each spectrum ([`visit_scans`] does that
//! transparently, and says so in [`ScanList::source`]). [`ScanFilter`] selects scans (MS level,
//! polarity, retention-time window, precursor m/z within a tolerance, charge, activation, filter
//! substring); [`list_scans`] counts every match and returns one page of them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reader::{Dataset, SpectrumView};
use crate::{Error, Result, Spectrum};

/// Everything about one spectrum except its peaks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScanHeader {
    /// Zero-based spectrum index within its run (`spectra --index`).
    pub index: u64,
    /// Scan number as the instrument counts it (`spectra --scan`).
    pub scan_number: u64,
    /// The spectrum's identifier in its file (mzML `id`), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    /// MS level: 1 for a full scan, 2 for a fragment (MS/MS) scan, and so on.
    pub ms_level: u32,
    /// Retention time in seconds; null when the file states none for this scan (such a scan
    /// never matches a retention-time filter).
    #[serde(default)]
    pub rt_s: Option<f64>,
    /// `positive`, `negative` or `unknown`.
    pub polarity: String,
    /// True when the stored peaks are centroids.
    pub centroided: bool,
    /// m/z of the isolated precursor ion (MS2 and above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// Charge state of the precursor, when the instrument determined it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_charge: Option<i32>,
    /// Intensity of the selected precursor ion, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_intensity: Option<f64>,
    /// Precursor isolation window `[lower, upper]` in m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_window_mz: Option<[f64; 2]>,
    /// Dissociation method (`CID`, `HCD`, `ETD`, ...), upper case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    /// Collision energy as recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision_energy: Option<f64>,
    /// Ion mobility 1/K0 in V·s/cm², when the scan or precursor has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inverse_reduced_mobility: Option<f64>,
    /// Instrument scan filter or scan description (e.g. Thermo `FTMS + p ESI d Full ms2 …`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_filter: Option<String>,
    /// Scan (acquisition) window `[lower, upper]` in m/z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_window_mz: Option<[f64; 2]>,
    /// Total ion current as the file records it for the scan (an index or header value; not
    /// recomputed from the peaks). Absent when the file keeps none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ion_current: Option<f64>,
    /// Base-peak m/z as the file records it for the scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_peak_mz: Option<f64>,
    /// Base-peak intensity as the file records it for the scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_peak_intensity: Option<f64>,
    /// Number of stored points (peaks or profile samples), when the header records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_count: Option<u64>,
    /// Format-specific scan metadata in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl From<Spectrum> for ScanHeader {
    fn from(s: Spectrum) -> Self {
        let point_count = if s.mz.is_empty() {
            None
        } else {
            Some(s.mz.len() as u64)
        };
        ScanHeader {
            index: s.index,
            scan_number: s.scan_number,
            native_id: s.native_id,
            ms_level: s.ms_level,
            rt_s: s.rt_s,
            polarity: s.polarity,
            centroided: s.centroided,
            precursor_mz: s.precursor_mz,
            precursor_charge: s.precursor_charge,
            precursor_intensity: s.precursor_intensity,
            isolation_window_mz: s.isolation_window_mz,
            activation: s.activation,
            collision_energy: s.collision_energy,
            inverse_reduced_mobility: s.inverse_reduced_mobility,
            scan_filter: s.scan_filter,
            scan_window_mz: s.scan_window_mz,
            total_ion_current: s.total_ion_current,
            base_peak_mz: s.base_peak_mz,
            base_peak_intensity: s.base_peak_intensity,
            point_count,
            extra: s.extra,
        }
    }
}

/// Which scans to list. Every condition that is set must hold.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScanFilter {
    /// Only this MS level (1 = full scans, 2 = MS/MS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_level: Option<u32>,
    /// Only `positive` or `negative` scans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,
    /// Retention time at least this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_min_s: Option<f64>,
    /// Retention time at most this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_max_s: Option<f64>,
    /// Precursor m/z near this value (within `precursor_tol_mz`, or `precursor_tol_ppm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// Absolute precursor tolerance in m/z (default 0.01 when neither tolerance is given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_tol_mz: Option<f64>,
    /// Relative precursor tolerance in ppm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precursor_tol_ppm: Option<f64>,
    /// Only precursors of this charge state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge: Option<i32>,
    /// Only this activation (`HCD`, `CID`, ...; case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    /// Only scans whose filter string contains this text (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_contains: Option<String>,
}

/// Default precursor tolerance (m/z) when a precursor is given without one.
pub const DEFAULT_PRECURSOR_TOL_MZ: f64 = 0.01;

impl ScanFilter {
    /// Is any condition set?
    pub fn is_empty(&self) -> bool {
        *self == ScanFilter::default()
    }

    /// Check the conditions for consistency (a usage error names the problem).
    pub fn validate(&self) -> Result<()> {
        if let Some(p) = &self.polarity
            && !matches!(p.as_str(), "positive" | "negative" | "unknown")
        {
            return Err(Error::Usage(format!(
                "polarity `{p}`: use positive, negative or unknown"
            )));
        }
        if let (Some(a), Some(b)) = (self.rt_min_s, self.rt_max_s)
            && a > b
        {
            return Err(Error::Usage(format!(
                "retention-time window {a}–{b} s is empty (minimum above maximum)"
            )));
        }
        if self.precursor_tol_mz.is_some() && self.precursor_tol_ppm.is_some() {
            return Err(Error::Usage(
                "give the precursor tolerance in m/z or in ppm, not both".into(),
            ));
        }
        for (name, v) in [
            ("precursor tolerance", self.precursor_tol_mz),
            ("precursor ppm tolerance", self.precursor_tol_ppm),
        ] {
            if v.is_some_and(|v| !(v.is_finite() && v >= 0.0)) {
                return Err(Error::Usage(format!(
                    "{name} must be a non-negative number"
                )));
            }
        }
        Ok(())
    }

    /// Does the scan pass every condition?
    pub fn matches(&self, h: &ScanHeader) -> bool {
        if self.ms_level.is_some_and(|l| l != h.ms_level) {
            return false;
        }
        if self.polarity.as_ref().is_some_and(|p| *p != h.polarity) {
            return false;
        }
        if (self.rt_min_s.is_some() || self.rt_max_s.is_some())
            && !h.rt_s.is_some_and(|rt| {
                self.rt_min_s.is_none_or(|t| rt >= t) && self.rt_max_s.is_none_or(|t| rt <= t)
            })
        {
            return false;
        }
        if let Some(target) = self.precursor_mz {
            let tol = match (self.precursor_tol_ppm, self.precursor_tol_mz) {
                (Some(ppm), _) => target.abs() * ppm * 1e-6,
                (None, Some(t)) => t,
                (None, None) => DEFAULT_PRECURSOR_TOL_MZ,
            };
            if !h.precursor_mz.is_some_and(|p| (p - target).abs() <= tol) {
                return false;
            }
        }
        if self.charge.is_some_and(|z| h.precursor_charge != Some(z)) {
            return false;
        }
        if let Some(a) = &self.activation
            && !h
                .activation
                .as_ref()
                .is_some_and(|x| x.eq_ignore_ascii_case(a))
        {
            return false;
        }
        if let Some(t) = &self.filter_contains {
            let t = t.to_ascii_lowercase();
            if !h
                .scan_filter
                .as_ref()
                .is_some_and(|f| f.to_ascii_lowercase().contains(&t))
            {
                return false;
            }
        }
        true
    }
}

/// Output of `spectra` (scan list): the matching scans of one run, one page of them listed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScanList {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Run index.
    pub run: u32,
    /// Spectra in the run.
    pub scan_count: u64,
    /// `headers` when the reader listed scan headers without decoding peaks, `spectra` when
    /// every spectrum had to be decoded to list it.
    pub source: String,
    /// The conditions applied (omitted when every scan is listed).
    #[serde(default, skip_serializing_if = "ScanFilter::is_empty")]
    pub filter: ScanFilter,
    /// Scans matching the conditions (all of them are counted, not only the page listed).
    pub matched: u64,
    /// Position of the first listed scan among the matching ones (0-based; `--offset`).
    pub offset: u64,
    /// Number of scans listed.
    pub returned: u64,
    /// True when more matching scans follow the page (`offset + returned < matched`).
    pub truncated: bool,
    /// Counts of the matching scans per MS level (`"1"`, `"2"`, ...).
    pub ms_level_counts: BTreeMap<String, u64>,
    /// Retention-time range of the matching scans in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_range_s: Option<[f64; 2]>,
    /// The listed scans, in spectrum-index order.
    pub scans: Vec<ScanHeader>,
}

/// How [`visit_scans`] obtained the headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderSource {
    /// The reader's header-only path (no peaks decoded).
    Headers,
    /// Every spectrum decoded (the reader has no header-only path).
    Spectra,
}

impl HeaderSource {
    /// `headers` or `spectra`.
    pub fn name(self) -> &'static str {
        match self {
            HeaderSource::Headers => "headers",
            HeaderSource::Spectra => "spectra",
        }
    }
}

/// Visit the header of every spectrum of run `run` in index order, through the reader's
/// header-only path when it has one, else by decoding each spectrum (the run's `scan_count`
/// from `info` bounds that walk; a file without that run is a usage error). `visit` returns
/// false to stop early.
pub fn visit_scans(
    ds: &mut dyn Dataset,
    run: u32,
    visit: &mut dyn FnMut(ScanHeader) -> bool,
) -> Result<HeaderSource> {
    if ds.visit_scan_headers(run, 0, visit)? {
        return Ok(HeaderSource::Headers);
    }
    let scan_count = run_scan_count(ds, run)?;
    for i in 0..scan_count {
        let mut s = ds.read_spectrum_view(run, i, SpectrumView::Primary)?;
        let n = s.mz.len() as u64;
        s.mz = Vec::new();
        s.intensity = Vec::new();
        let mut h = ScanHeader::from(s);
        h.point_count = Some(n);
        if !visit(h) {
            break;
        }
    }
    Ok(HeaderSource::Spectra)
}

/// Number of spectra of run `run` (`info` → `spectra[run].scan_count`), or a usage error
/// naming what the file holds.
pub(crate) fn run_scan_count(ds: &mut dyn Dataset, run: u32) -> Result<u64> {
    let info = ds.info()?;
    info.spectra
        .iter()
        .find(|s| s.index == run)
        .map(|s| s.scan_count)
        .ok_or_else(|| {
            if info.spectra.is_empty() {
                Error::Usage(format!(
                    "{} holds no mass spectra (`info` lists what it holds)",
                    info.path
                ))
            } else {
                Error::Usage(format!(
                    "run {run} out of range ({} run{})",
                    info.spectra.len(),
                    if info.spectra.len() == 1 { "" } else { "s" }
                ))
            }
        })
}

/// The scans of run `run` matching `filter`: all of them counted (per MS level, RT range), the
/// `limit` matches from the `offset`-th listed. `ScanPage::scanned` is the run's scan count.
pub fn list_scans(
    ds: &mut dyn Dataset,
    run: u32,
    filter: &ScanFilter,
    offset: u64,
    limit: u64,
) -> Result<(HeaderSource, ScanPage)> {
    filter.validate()?;
    let mut page = ScanPage::default();
    let src = visit_scans(ds, run, &mut |h| {
        page.scanned += 1;
        if !filter.matches(&h) {
            return true;
        }
        page.add(h, offset, limit);
        true
    })?;
    Ok((src, page))
}

/// [`list_scans`] as the `spectra` scan-list output: `path` and `format` name the input.
pub fn scan_list(
    ds: &mut dyn Dataset,
    path: &str,
    format: &str,
    run: u32,
    filter: &ScanFilter,
    offset: u64,
    limit: u64,
) -> Result<ScanList> {
    let (src, page) = list_scans(ds, run, filter, offset, limit)?;
    let returned = page.scans.len() as u64;
    Ok(ScanList {
        path: path.to_string(),
        format: format.to_string(),
        run,
        scan_count: page.scanned,
        source: src.name().into(),
        filter: filter.clone(),
        matched: page.matched,
        offset,
        returned,
        truncated: offset.saturating_add(returned) < page.matched,
        ms_level_counts: page.ms_level_counts,
        rt_range_s: page.rt_range_s,
        scans: page.scans,
    })
}

/// Matching scans counted and one page of them kept (see [`list_scans`]).
#[derive(Debug, Clone, Default)]
pub struct ScanPage {
    /// Scans visited (every scan of the run, once the walk is complete).
    pub scanned: u64,
    /// Scans matched.
    pub matched: u64,
    /// Counts per MS level.
    pub ms_level_counts: BTreeMap<String, u64>,
    /// Retention-time range of the matches in seconds.
    pub rt_range_s: Option<[f64; 2]>,
    /// The page.
    pub scans: Vec<ScanHeader>,
}

impl ScanPage {
    /// Count a matching scan and keep it when it falls in the page `offset..offset + limit`.
    pub fn add(&mut self, h: ScanHeader, offset: u64, limit: u64) {
        *self
            .ms_level_counts
            .entry(h.ms_level.to_string())
            .or_insert(0) += 1;
        if let Some(rt) = h.rt_s.filter(|t| t.is_finite()) {
            self.rt_range_s = Some(match self.rt_range_s {
                Some([a, b]) => [a.min(rt), b.max(rt)],
                None => [rt, rt],
            });
        }
        if self.matched >= offset && self.matched - offset < limit {
            self.scans.push(h);
        }
        self.matched += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(index: u64, ms_level: u32, rt_s: f64, mz: Option<f64>, z: Option<i32>) -> ScanHeader {
        ScanHeader {
            index,
            scan_number: index + 1,
            ms_level,
            rt_s: Some(rt_s),
            polarity: "positive".into(),
            precursor_mz: mz,
            precursor_charge: z,
            activation: mz.map(|_| "HCD".into()),
            scan_filter: Some(if ms_level == 1 {
                "FTMS + p ESI Full ms [100-1000]".into()
            } else {
                "FTMS + c ESI d Full ms2 500.25@hcd30.00".into()
            }),
            ..ScanHeader::default()
        }
    }

    #[test]
    fn filters_combine() {
        let scans = [
            h(0, 1, 10.0, None, None),
            h(1, 2, 10.5, Some(500.25), Some(2)),
            h(2, 2, 11.0, Some(500.2551), Some(3)),
            h(3, 2, 30.0, Some(612.3), Some(3)),
        ];
        let f = ScanFilter {
            ms_level: Some(2),
            charge: Some(3),
            ..ScanFilter::default()
        };
        assert_eq!(scans.iter().filter(|s| f.matches(s)).count(), 2);
        let f = ScanFilter {
            precursor_mz: Some(500.25),
            ..ScanFilter::default()
        };
        assert_eq!(scans.iter().filter(|s| f.matches(s)).count(), 2);
        let f = ScanFilter {
            precursor_mz: Some(500.25),
            precursor_tol_ppm: Some(5.0),
            ..ScanFilter::default()
        };
        assert_eq!(scans.iter().filter(|s| f.matches(s)).count(), 1);
        let f = ScanFilter {
            rt_min_s: Some(10.2),
            rt_max_s: Some(20.0),
            activation: Some("hcd".into()),
            filter_contains: Some("MS2".into()),
            ..ScanFilter::default()
        };
        assert_eq!(scans.iter().filter(|s| f.matches(s)).count(), 2);
        assert!(
            ScanFilter {
                rt_min_s: Some(5.0),
                rt_max_s: Some(1.0),
                ..ScanFilter::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            ScanFilter {
                polarity: Some("pos".into()),
                ..ScanFilter::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn pages_count_every_match() {
        let mut p = ScanPage::default();
        for i in 0..10 {
            p.add(h(i, 1 + (i % 2) as u32, i as f64, None, None), 3, 4);
        }
        assert_eq!(p.matched, 10);
        assert_eq!(
            p.scans.iter().map(|s| s.index).collect::<Vec<_>>(),
            [3, 4, 5, 6]
        );
        assert_eq!(p.ms_level_counts["1"], 5);
        assert_eq!(p.rt_range_s, Some([0.0, 9.0]));
    }

    /// A scan whose file states no retention time: listed with a null time, left out of the
    /// time range, and never matched by a retention-time window.
    #[test]
    fn absent_retention_time() {
        let mut none = h(5, 1, 0.0, None, None);
        none.rt_s = None;
        let f = ScanFilter {
            rt_min_s: Some(-1.0),
            ..ScanFilter::default()
        };
        assert!(!f.matches(&none));
        assert!(ScanFilter::default().matches(&none));
        let mut p = ScanPage::default();
        p.add(h(0, 1, 12.0, None, None), 0, 10);
        p.add(none.clone(), 0, 10);
        assert_eq!(p.rt_range_s, Some([12.0, 12.0]));
        assert_eq!(p.matched, 2);
        let json = serde_json::to_value(&none).unwrap();
        assert!(json["rt_s"].is_null(), "{json}");
    }
}
