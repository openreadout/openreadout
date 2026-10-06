//! The analyses as batch measures: `peaks`, `chromatogram`, `assay`, `nmr-peaks`,
//! `ephys-features`, `spikes`, `qpcr`, and `stats` per well (`well-stats`). Each runs the same library call as the
//! single-file command (and its MCP tool) with the same JSON options, then turns the report's
//! records into rows: the numbers are those of the single-file output, cell for cell.
//!
//! Options are the MCP tool's arguments without `file` (`{"mz": [195.0877], "ppm": 10}` for
//! `peaks`, `{"analysis": "curve", "model": "4pl"}` for `assay`), plus `rows`, which picks the
//! record list when a report has several (see [`AnalysisMeasure::rows_kinds`]). Unknown option
//! names are an error that lists the valid ones.

use std::collections::BTreeSet;

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Result};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as J};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};

/// The analysis measures by name (the `kind`s of `openreadout_analyze` other than `gate`,
/// which is a built-in measure).
pub const ANALYSIS_MEASURES: [&str; 7] = [
    "peaks",
    "chromatogram",
    "assay",
    "nmr-peaks",
    "ephys-features",
    "spikes",
    "qpcr",
];

/// `qpcr` options (`openreadout_analyze` kind `qpcr`).
#[derive(Debug, Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct QpcrQuery {
    /// Only this well (`A1`).
    pub well: Option<String>,
    /// Only this target, case-insensitive.
    pub target: Option<String>,
    /// Only this sample, case-insensitive.
    pub sample: Option<String>,
    /// Only this run.
    pub run: Option<String>,
    /// Also compute our own threshold Cq.
    #[serde(default)]
    pub compute_cq: bool,
    /// Threshold for `compute_cq`.
    pub threshold: Option<f64>,
    /// Baseline window for `compute_cq`, first cycle.
    pub baseline_start: Option<u32>,
    /// Baseline window for `compute_cq`, last cycle.
    pub baseline_end: Option<u32>,
    /// ΔΔCq relative quantification (rows: one per sample × target).
    #[serde(default)]
    pub ddcq: bool,
    /// ΔΔCq reference targets; default: the file's.
    #[serde(default)]
    pub reference_targets: Vec<String>,
    /// ΔΔCq control (calibrator) sample; default: the file's.
    pub control_sample: Option<String>,
    /// Standard curve per target (rows: one per target).
    #[serde(default)]
    pub standard_curve: bool,
    /// Cq for undetermined results in means and ΔΔCq; default: left out and counted.
    pub undetermined_cq: Option<f64>,
}

/// Options of `stats` per well (`--per well|field`; `openreadout_stats` per=well).
#[derive(Debug, Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WellStatsQuery {
    /// Plane selection (`c=0`).
    #[serde(default)]
    pub select: Vec<String>,
    /// Only these wells (`C05`).
    #[serde(default)]
    pub wells: Vec<String>,
    /// One row per well, field and channel.
    #[serde(default)]
    pub per_field: bool,
    /// Pyramid level.
    #[serde(default)]
    pub level: u32,
}

/// Which command, with its parsed options.
#[derive(Debug)]
enum Kind {
    Peaks(Box<openreadout_quant::api::PeaksQuery>),
    Chromatogram(Box<openreadout_quant::api::ChromatogramQuery>),
    Assay(Box<openreadout_assay::AssayRequest>),
    Nmr(Box<openreadout_signal::api::NmrQuery>),
    Ephys(Box<openreadout_signal::api::EphysQuery>),
    Spikes(Box<openreadout_signal::api::SpikesQuery>),
    Qpcr(Box<QpcrQuery>),
    WellStats(Box<WellStatsQuery>),
}

/// One analysis command run per data set, its records as rows.
#[derive(Debug)]
pub struct AnalysisMeasure {
    id: &'static str,
    kind: Kind,
    rows: String,
    fingerprint: String,
}

/// The property names a query type accepts (from its JSON Schema, following `allOf` and
/// `$ref`, which is how `#[serde(flatten)]` fields appear).
/// The argument names a schema accepts: its properties, through references and `allOf`/`anyOf`/
/// `oneOf` (flattened structs).
pub fn accepted_keys<T: JsonSchema>() -> BTreeSet<String> {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(J::Null);
    let mut keys = BTreeSet::new();
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .cloned()
        .unwrap_or(J::Null);
    walk_keys(&schema, &defs, &mut keys, 0);
    keys
}

fn walk_keys(v: &J, defs: &J, keys: &mut BTreeSet<String>, depth: u32) {
    if depth > 8 {
        return;
    }
    if let Some(p) = v.get("properties").and_then(J::as_object) {
        keys.extend(p.keys().cloned());
    }
    if let Some(r) = v.get("$ref").and_then(J::as_str)
        && let Some(name) = r.rsplit('/').next()
        && let Some(d) = defs.get(name)
    {
        walk_keys(d, defs, keys, depth + 1);
    }
    for k in ["allOf", "anyOf", "oneOf"] {
        if let Some(a) = v.get(k).and_then(J::as_array) {
            for x in a {
                walk_keys(x, defs, keys, depth + 1);
            }
        }
    }
}

/// Options of `measure` as `T`; an option `T` does not take is an error listing the ones it does.
pub fn parse_options<T: DeserializeOwned + JsonSchema>(
    measure: &str,
    options: &Map<String, J>,
) -> Result<T> {
    let keys = accepted_keys::<T>();
    let unknown: Vec<&String> = options.keys().filter(|k| !keys.contains(*k)).collect();
    if !unknown.is_empty() {
        return Err(Error::Usage(format!(
            "`{measure}` has no option {}; it takes: {}",
            unknown
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", "),
            keys.iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    serde_json::from_value(J::Object(options.clone()))
        .map_err(|e| Error::Usage(format!("`{measure}` options: {e}")))
}

impl AnalysisMeasure {
    /// The `rows` values each measure accepts (the first is the default, except where the
    /// options pick one: compounds for `peaks`, integrals for `nmr-peaks`, `ddcq` and
    /// `standard_curve` for `qpcr`, the analysis for `assay`).
    pub fn rows_kinds(measure: &str) -> &'static [&'static str] {
        match measure {
            "peaks" => &["peak", "compound", "chromatogram", "band", "region"],
            "chromatogram" => &["chromatogram"],
            "assay" => &[
                "auto",
                "wells",
                "samples",
                "compounds",
                "kinetics",
                "growth",
                "quality",
            ],
            "nmr-peaks" => &["peak", "integral", "spectrum"],
            "ephys-features" => &["sweep", "cell", "spike"],
            "spikes" => &["channel"],
            "qpcr" => &["record", "rq", "standard_curve"],
            "well-stats" => &["well"],
            _ => &[],
        }
    }

    /// Build `measure` from its JSON options (the single-file MCP tool's arguments without
    /// `file`, plus `rows`). `None` when `measure` is not an analysis measure.
    pub fn build(measure: &str, options: &Map<String, J>) -> Option<Result<Self>> {
        let id = *ANALYSIS_MEASURES.iter().find(|m| **m == measure)?;
        Some(Self::build_known(id, options))
    }

    /// `stats` per well of a multi-well plate, from [`WellStatsQuery`] options.
    pub fn well_stats(options: &Map<String, J>) -> Result<Self> {
        Self::build_known("well-stats", options)
    }

    fn build_known(id: &'static str, options: &Map<String, J>) -> Result<Self> {
        let mut opts = options.clone();
        let rows_given = match opts.remove("rows") {
            None => None,
            Some(J::String(s)) => Some(s),
            Some(other) => {
                return Err(Error::Usage(format!(
                    "`rows` must be a string, got {other}"
                )));
            }
        };
        let kind = match id {
            "peaks" => Kind::Peaks(Box::new(parse_options(id, &opts)?)),
            "chromatogram" => Kind::Chromatogram(Box::new(parse_options(id, &opts)?)),
            "assay" => Kind::Assay(Box::new(parse_options(id, &opts)?)),
            "nmr-peaks" => Kind::Nmr(Box::new(parse_options(id, &opts)?)),
            "ephys-features" => Kind::Ephys(Box::new(parse_options(id, &opts)?)),
            "spikes" => Kind::Spikes(Box::new(parse_options(id, &opts)?)),
            "qpcr" => Kind::Qpcr(Box::new(parse_options(id, &opts)?)),
            _ => Kind::WellStats(Box::new(parse_options(id, &opts)?)),
        };
        // Validate requests up front, so a bad option is one usage error, not an error row
        // per file.
        match &kind {
            Kind::Nmr(q) => drop(q.request()?),
            Kind::Ephys(q) => drop(q.request()?),
            Kind::Spikes(q) => drop(q.request()?),
            Kind::Peaks(q) => drop(q.source.request()?),
            Kind::Chromatogram(q) => drop(q.source.request()?),
            _ => {}
        }
        let kinds = Self::rows_kinds(id);
        let rows = match rows_given {
            Some(r) if kinds.contains(&r.as_str()) => r,
            Some(r) => {
                return Err(Error::Usage(format!(
                    "`{id}` rows `{r}`: use {}",
                    kinds.join(", ")
                )));
            }
            None => match &kind {
                Kind::Peaks(q) if !q.compounds.is_empty() => "compound".into(),
                Kind::Peaks(q) if !q.x_range.is_empty() => "region".into(),
                Kind::Nmr(q) if !q.integrate.is_empty() => "integral".into(),
                Kind::Qpcr(q) if q.ddcq => "rq".into(),
                Kind::Qpcr(q) if q.standard_curve => "standard_curve".into(),
                _ => kinds[0].into(),
            },
        };
        let fingerprint = format!(
            "{id}:{rows}:{}",
            serde_json::to_string(&J::Object(opts)).unwrap_or_default()
        );
        Ok(AnalysisMeasure {
            id,
            kind,
            rows,
            fingerprint,
        })
    }
}

/// Put a JSON value's scalar leaves into `r`: nested objects as `parent_child`, short lists of
/// scalars (≤ 16) as `a;b;c` text; longer lists and lists of objects are left out (they are
/// per-point arrays, available from the single-file command).
fn flatten(r: &mut Row, prefix: &str, v: &J, skip: &[&str], depth: u32) {
    let name = |k: &str| {
        if prefix.is_empty() {
            k.to_string()
        } else {
            format!("{prefix}_{k}")
        }
    };
    match v {
        J::Object(m) => {
            if depth > 3 {
                return;
            }
            for (k, x) in m {
                if prefix.is_empty() && skip.contains(&k.as_str()) {
                    continue;
                }
                flatten(r, &name(k), x, skip, depth + 1);
            }
        }
        J::Array(a) => {
            if a.len() <= 16 && a.iter().all(|x| !x.is_object() && !x.is_array()) {
                let parts: Vec<String> = a
                    .iter()
                    .map(|x| match x {
                        J::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                if !parts.is_empty() {
                    r.set(prefix, parts.join(";"));
                }
            }
        }
        J::Null => {}
        J::Bool(b) => {
            r.set(prefix, *b);
        }
        J::Number(n) => {
            let cell = match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::float_opt(n.as_f64()),
            };
            r.set(prefix, cell);
        }
        J::String(s) => {
            r.set(prefix, s.as_str());
        }
    }
}

/// Rows from a list of records: each record flattened, `skip` left out.
fn records<T: Serialize>(items: &[T], skip: &[&str]) -> Vec<Row> {
    items
        .iter()
        .map(|x| {
            let mut r = Row::new();
            flatten(
                &mut r,
                "",
                &serde_json::to_value(x).unwrap_or(J::Null),
                skip,
                0,
            );
            r
        })
        .collect()
}

/// Rows from a list of records, each led by a zero-based counter column `key`.
fn numbered<T: Serialize>(key: &str, items: &[T], skip: &[&str]) -> Vec<Row> {
    records(items, skip)
        .into_iter()
        .zip(0u64..)
        .map(|(r, i)| {
            let mut n = Row::new().with(key, i);
            n.cells.extend(r.cells);
            n
        })
        .collect()
}

/// One row from one object.
fn single<T: Serialize>(x: &T, skip: &[&str]) -> Row {
    records(std::slice::from_ref(x), skip)
        .pop()
        .unwrap_or_default()
}

fn ctx_rows<T>(
    it: &mut Item<'_>,
    f: impl FnOnce(&mut dyn Dataset, &openreadout_core::FileInfo, &ReadContext<'_>) -> Result<T>,
) -> Result<T> {
    let reg = it.registry;
    let path = it.path;
    let opener = || -> Result<Box<dyn Dataset>> { reg.open(path).map(|(_, d)| d) };
    f(
        it.dataset,
        it.info,
        &ReadContext {
            opener: Some(&opener),
            progress: None,
        },
    )
}

impl Measure for AnalysisMeasure {
    fn id(&self) -> &'static str {
        self.id
    }
    fn grain(&self) -> Vec<String> {
        let g: &[&str] = match (self.id, self.rows.as_str()) {
            ("peaks", "peak") => &["chromatogram", "number"],
            ("peaks", "compound") => &["compound"],
            ("peaks", "band") => &["spectrum", "number"],
            ("peaks", "region") => &["spectrum", "number"],
            ("peaks" | "chromatogram", _) => &["chromatogram"],
            ("assay", "samples") => &["group"],
            ("assay", "compounds") => &["compound"],
            ("assay", "quality") => &[],
            ("assay", _) => &["well"],
            ("nmr-peaks", "peak") => &["peak"],
            ("nmr-peaks", "integral") => &["region"],
            ("nmr-peaks", _) | ("ephys-features", "cell") => &[],
            ("ephys-features", "sweep") => &["sweep"],
            ("ephys-features", _) => &["sweep", "index"],
            ("spikes", _) => &["channel"],
            ("qpcr", "rq") => &["sample", "target"],
            ("qpcr", "standard_curve") => &["target"],
            ("qpcr", _) => &["run", "well", "target"],
            _ => &["well", "field", "c"],
        };
        g.iter().map(|s| (*s).to_string()).collect()
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        match self.id {
            "peaks" => vec![
                ColumnDoc::key(
                    "chromatogram",
                    "chromatogram label (TIC, XIC m/z …, trace name)",
                ),
                ColumnDoc::key("number", "peak number in retention order, from 1"),
                ColumnDoc::key("compound", "compound name from the compound list"),
                ColumnDoc::unit("rt_min", "min", "retention time of the apex"),
                ColumnDoc::value("area", "peak area in `area_unit`"),
                ColumnDoc::value("area_unit", "signal×min, or signal×s with area_seconds"),
                ColumnDoc::value("area_percent", "share of the total peak area, %"),
                ColumnDoc::key(
                    "spectrum",
                    "spectrum label (trace name) of a band or region",
                ),
                ColumnDoc::value("x", "band apex on the spectrum's axis (x_unit)"),
                ColumnDoc::value(
                    "from",
                    "region lower end on the axis (x_unit); `to` the upper end",
                ),
                ColumnDoc::value(
                    "area_no_baseline",
                    "region integral of the signal itself (area uses the baseline)",
                ),
            ],
            "chromatogram" => vec![
                ColumnDoc::key("chromatogram", "chromatogram label"),
                ColumnDoc::unit("apex_rt_min", "min", "retention time of the highest point"),
                ColumnDoc::value("apex_intensity", "highest point"),
                ColumnDoc::value("integral", "trapezoid area, intensity×min, no baseline"),
            ],
            "nmr-peaks" => vec![
                ColumnDoc::key("peak", "peak number, tallest-first as reported, from 0"),
                ColumnDoc::key("region", "integration region, in the order given, from 0"),
                ColumnDoc::unit("ppm", "ppm", "chemical shift"),
            ],
            "ephys-features" => vec![ColumnDoc::key("sweep", "sweep index, from 0")],
            "spikes" => vec![
                ColumnDoc::key("channel", "channel index, from 0"),
                ColumnDoc::unit("rate_hz", "Hz", "spikes per second"),
            ],
            "qpcr" => vec![
                ColumnDoc::key("well", "well (A1)"),
                ColumnDoc::key("target", "target (gene, assay)"),
                ColumnDoc::value("cq", "the vendor's Cq (Ct)"),
            ],
            "well-stats" => vec![
                ColumnDoc::key("well", "plate well (C05)"),
                ColumnDoc::key("c", "channel index, from 0"),
            ],
            _ => vec![ColumnDoc::key("well", "plate well (A1)")],
        }
    }
    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        let rows = self.rows.as_str();
        Ok(match &self.kind {
            Kind::Peaks(q) => {
                let out = ctx_rows(it, |ds, info, ctx| {
                    openreadout_quant::api::peaks(ds, info, q, ctx)
                })?;
                match rows {
                    "compound" => records(&out.compounds, &["path"]),
                    "chromatogram" => out
                        .chromatograms
                        .iter()
                        .map(|c| {
                            let mut r = Row::new().with("chromatogram", c.label.as_str());
                            let v = serde_json::to_value(c).unwrap_or(J::Null);
                            flatten(
                                &mut r,
                                "",
                                &v,
                                &["label", "peaks", "manual", "method", "picked", "notes"],
                                0,
                            );
                            if let Some(p) = c
                                .main_peak
                                .and_then(|n| c.peaks.iter().find(|p| p.number == n))
                            {
                                let pv = serde_json::to_value(p).unwrap_or(J::Null);
                                flatten(&mut r, "main", &pv, &[], 1);
                            }
                            r
                        })
                        .collect(),
                    "band" => records(&out.band_rows(), &["path"]),
                    "region" => records(&out.region_rows(), &["path"]),
                    _ if out.chromatograms.is_empty() && !out.spectra.is_empty() => {
                        records(&out.band_rows(), &["path"])
                    }
                    _ => records(&out.peak_rows(), &["path"]),
                }
            }
            Kind::Chromatogram(q) => {
                let out = ctx_rows(it, |ds, info, ctx| {
                    openreadout_quant::api::chromatogram(ds, info, q, ctx)
                })?;
                out.chromatograms
                    .iter()
                    .map(|c| {
                        let mut r = Row::new().with("chromatogram", c.label.as_str());
                        let v = serde_json::to_value(c).unwrap_or(J::Null);
                        flatten(&mut r, "", &v, &["label", "rt_min", "intensity"], 0);
                        r
                    })
                    .collect()
            }
            Kind::Assay(req) => {
                let out = openreadout_assay::analyze_file(it.registry, it.path, req)?;
                let pick = if rows == "auto" {
                    match req.analysis {
                        openreadout_assay::Analysis::DoseResponse => "compounds",
                        openreadout_assay::Analysis::Kinetics => "kinetics",
                        openreadout_assay::Analysis::Growth => "growth",
                        openreadout_assay::Analysis::Qc => "quality",
                        _ => "wells",
                    }
                } else {
                    rows
                };
                match pick {
                    "samples" => records(&out.samples, &[]),
                    "compounds" => records(&out.compounds, &[]),
                    "kinetics" => records(&out.kinetics, &[]),
                    "growth" => records(&out.growth, &[]),
                    "quality" => out.quality.iter().map(|q| single(q, &[])).collect(),
                    _ => records(&out.wells, &[]),
                }
            }
            Kind::Nmr(q) => {
                let req = q.request()?;
                let out = openreadout_signal::nmr::analyze(it.dataset, it.info, &req)?;
                match rows {
                    "integral" => numbered("region", &out.integrals, &[]),
                    "spectrum" => vec![single(
                        &out,
                        &[
                            "path",
                            "format",
                            "processing",
                            "axis",
                            "peak_options",
                            "peaks",
                            "integrals",
                            "notes",
                        ],
                    )],
                    _ => numbered("peak", &out.peaks, &[]),
                }
            }
            Kind::Ephys(q) => {
                let mut req = q.request()?;
                if rows == "spike" && q.max_spikes.is_none() {
                    req.max_spikes = usize::MAX;
                }
                let out = openreadout_signal::ephys::analyze_cell(it.dataset, it.info, &req)?;
                match rows {
                    "cell" => {
                        let mut r = Row::new();
                        r.set("clamp_mode", out.clamp_mode.as_str());
                        r.set("spike_count_total", out.spike_count_total as u64);
                        let v = serde_json::to_value(&out.cell).unwrap_or(J::Null);
                        flatten(&mut r, "", &v, &[], 0);
                        vec![r]
                    }
                    "spike" => records(&out.spikes, &[]),
                    _ => records(&out.sweeps, &[]),
                }
            }
            Kind::Spikes(q) => {
                let req = q.request()?;
                let out =
                    openreadout_signal::ephys::analyze_extracellular(it.dataset, it.info, &req)?;
                records(&out.channels, &["times", "times_truncated"])
            }
            Kind::Qpcr(q) => {
                let ds = openreadout_qpcr::open_qpcr(it.registry, it.path)?;
                let mut req = openreadout_qpcr::QpcrReportRequest::default();
                req.well.clone_from(&q.well);
                req.target.clone_from(&q.target);
                req.sample.clone_from(&q.sample);
                req.run.clone_from(&q.run);
                req.compute_cq = q.compute_cq;
                req.threshold = q.threshold;
                req.baseline = match (q.baseline_start, q.baseline_end) {
                    (Some(s), Some(e)) if s >= 1 && e > s => Some((s, e)),
                    (None, None) => None,
                    _ => {
                        return Err(Error::Usage(
                            "baseline_start and baseline_end go together, with 1 <= start < end"
                                .into(),
                        ));
                    }
                };
                req.relative = q.ddcq || rows == "rq";
                req.reference_targets.clone_from(&q.reference_targets);
                req.control_sample.clone_from(&q.control_sample);
                req.standard_curve = q.standard_curve || rows == "standard_curve";
                req.undetermined_cq = q.undetermined_cq;
                let out = openreadout_qpcr::qpcr_report(&ds, &req)?;
                match rows {
                    "rq" => records(&out.relative_quantities, &[]),
                    "standard_curve" => records(&out.standard_curves, &[]),
                    _ => records(&out.records, &[]),
                }
            }
            Kind::WellStats(q) => {
                let mut req = openreadout_core::plate::WellStatsRequest::default();
                req.select.clone_from(&q.select);
                req.wells.clone_from(&q.wells);
                req.per_field = q.per_field;
                req.level = q.level;
                let out = ctx_rows(it, |ds, info, ctx| {
                    openreadout_core::plate::well_stats(ds, info, &req, ctx)
                })?;
                records(&out.rows, &[])
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(v: J) -> Map<String, J> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn options_are_checked_by_name() {
        let e = AnalysisMeasure::build("peaks", &opts(serde_json::json!({"mzz": [1.0]})))
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(e.contains("`mzz`") && e.contains("mz"), "{e}");
        // Flattened source fields are accepted.
        let m = AnalysisMeasure::build(
            "peaks",
            &opts(serde_json::json!({"mz": [195.0877], "ppm": 10, "rows": "chromatogram"})),
        )
        .unwrap()
        .unwrap();
        assert_eq!(m.grain(), vec!["chromatogram".to_string()]);
        let m = AnalysisMeasure::build("qpcr", &opts(serde_json::json!({"ddcq": true})))
            .unwrap()
            .unwrap();
        assert_eq!(m.rows, "rq");
        assert!(AnalysisMeasure::build("stats", &Map::new()).is_none());
        let e = AnalysisMeasure::build("spikes", &opts(serde_json::json!({"rows": "peak"})))
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(e.contains("channel"), "{e}");
        let e = AnalysisMeasure::build("assay", &opts(serde_json::json!({"analysis": "nope"})))
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(e.contains("assay"), "{e}");
    }

    #[test]
    fn flatten_keeps_scalars_and_short_lists() {
        let mut r = Row::new();
        flatten(
            &mut r,
            "",
            &serde_json::json!({"a": 1, "b": {"c": 2.5, "d": null}, "tm": [80.1, 85.2], "long": vec![0; 20], "objs": [{"x": 1}], "skip": 3}),
            &["skip"],
            0,
        );
        assert_eq!(r.get("a"), Some(&Value::Int(1)));
        assert_eq!(r.get("b_c"), Some(&Value::Float(2.5)));
        assert_eq!(r.get("tm"), Some(&Value::Text("80.1;85.2".into())));
        assert!(r.get("b_d").is_none() && r.get("long").is_none() && r.get("objs").is_none());
        assert!(r.get("skip").is_none());
    }
}
