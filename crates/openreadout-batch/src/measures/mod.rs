//! The built-in measures: `stats`, `trace`, `table`, `gate`, `info` and `scans`, and the
//! analyses (`peaks`, `chromatogram`, `assay`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr`;
//! `stats` per well) through [`AnalysisMeasure`].

mod analysis;
mod gate;
mod info;
mod scans;
mod stats;
mod table;
mod trace;

pub use analysis::{
    ANALYSIS_MEASURES, AnalysisMeasure, QpcrQuery, WellStatsQuery, accepted_keys, parse_options,
};
pub use gate::GateMeasure;
pub use info::{DEFAULT_INFO_FIELDS, InfoMeasure};
pub use scans::ScansMeasure;
pub use stats::{StatsMeasure, StatsPer};
pub use table::TableMeasure;
pub use trace::TraceMeasure;

use std::path::PathBuf;

use openreadout_core::model::{FileInfo, ImageInfo};
use openreadout_core::{Error, Result};
use openreadout_fcs::analysis::{
    CompensationChoice, TableOptions, TransformChoice, parse_transform_spec,
};

use crate::measure::Measure;

/// A measure and its options by name, as the MCP server and the Python package pass them.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MeasureSpec {
    /// `stats`, `trace`, `table`, `gate`, `info`, `scans` or an analysis.
    pub measure: String,
    /// stats: only this image.
    pub image: Option<u32>,
    /// stats: plane selection.
    pub select: Vec<String>,
    /// stats: pyramid level.
    pub level: u32,
    /// stats: `channel` (default), `image`, `plane`, `well` or `field`.
    pub per: Option<String>,
    /// stats: maximum-intensity projection first, along `z` or `t`.
    pub mip: Option<String>,
    /// trace: only this trace.
    pub trace: Option<u32>,
    /// trace: only this sweep.
    pub sweep: Option<u32>,
    /// trace: only these channels.
    pub channels: Vec<u32>,
    /// table, gate: FCS data set / table index.
    pub table: Option<u32>,
    /// table: only these parameters.
    pub parameters: Vec<String>,
    /// table (FCS): `auto`, `fcs` or `gating`.
    pub compensate: Option<String>,
    /// table (FCS): transform spec (`logicle`, `arcsinh-cofactor:5`, `workspace`).
    pub transform: Option<String>,
    /// gate (and table with gating): FlowJo workspace or Gating-ML file.
    pub gating_file: Option<PathBuf>,
    /// gate: workspace sample.
    pub sample: Option<String>,
    /// gate: populations to report.
    pub populations: Vec<String>,
    /// gate: parameters whose population medians are reported.
    pub medians: Vec<String>,
    /// info: fields.
    pub fields: Vec<String>,
    /// stats per well or field: only these wells.
    pub wells: Vec<String>,
    /// The analysis measures (`peaks`, `chromatogram`, `assay`, `nmr-peaks`,
    /// `ephys-features`, `spikes`, `qpcr`) and `scans`: the arguments of that MCP tool, plus
    /// `rows` (which record list becomes rows).
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// Options of the built-in measures by name (`openreadout batch stats DIR --set image=0`).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BuiltinOptions {
    image: Option<u32>,
    #[serde(default)]
    select: Vec<String>,
    #[serde(default)]
    level: u32,
    per: Option<String>,
    mip: Option<String>,
    trace: Option<u32>,
    sweep: Option<u32>,
    #[serde(default)]
    channels: Vec<u32>,
    table: Option<u32>,
    #[serde(default)]
    parameters: Vec<String>,
    compensate: Option<String>,
    transform: Option<String>,
    workspace: Option<PathBuf>,
    gatingml: Option<PathBuf>,
    sample: Option<String>,
    #[serde(default)]
    populations: Vec<String>,
    #[serde(default)]
    medians: Vec<String>,
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    wells: Vec<String>,
}

/// A spec from a measure name and options given by name (the CLI `batch` command): the
/// analysis measures keep them as `options`; the built-in ones take `image`, `select`, `level`,
/// `per`, `trace`, `sweep`, `channels`, `table`, `parameters`, `compensate`, `transform`,
/// `workspace`/`gatingml`, `sample`, `populations`, `medians`, `fields`.
pub fn spec_from_options(
    measure: &str,
    options: serde_json::Map<String, serde_json::Value>,
) -> Result<MeasureSpec> {
    if ANALYSIS_MEASURES.contains(&measure) || measure == "scans" {
        return Ok(MeasureSpec {
            measure: measure.to_string(),
            options,
            ..MeasureSpec::default()
        });
    }
    let o: BuiltinOptions = serde_json::from_value(serde_json::Value::Object(options))
        .map_err(|e| Error::Usage(format!("`{measure}` options: {e}")))?;
    Ok(MeasureSpec {
        measure: measure.to_string(),
        image: o.image,
        select: o.select,
        level: o.level,
        per: o.per,
        mip: o.mip,
        trace: o.trace,
        sweep: o.sweep,
        channels: o.channels,
        table: o.table,
        parameters: o.parameters,
        compensate: o.compensate,
        transform: o.transform,
        gating_file: o.workspace.or(o.gatingml),
        sample: o.sample,
        populations: o.populations,
        medians: o.medians,
        fields: o.fields,
        wells: o.wells,
        options: serde_json::Map::new(),
    })
}

/// Build the measure `spec` names.
pub fn build(spec: &MeasureSpec) -> Result<Box<dyn Measure>> {
    if spec.measure == "scans" {
        return Ok(Box::new(ScansMeasure::from_options(&spec.options)?));
    }
    if let Some(m) = AnalysisMeasure::build(&spec.measure, &spec.options) {
        return Ok(Box::new(m?));
    }
    if !spec.options.is_empty() {
        return Err(Error::Usage(format!(
            "`{}` takes no `options`; they are for {}",
            spec.measure,
            ANALYSIS_MEASURES.join(", ")
        )));
    }
    if spec.measure == "stats" && matches!(spec.per.as_deref(), Some("well" | "field")) {
        let mut o = serde_json::Map::new();
        o.insert("select".into(), serde_json::json!(spec.select));
        o.insert("wells".into(), serde_json::json!(spec.wells));
        o.insert(
            "per_field".into(),
            serde_json::json!(spec.per.as_deref() == Some("field")),
        );
        o.insert("level".into(), serde_json::json!(spec.level));
        return Ok(Box::new(AnalysisMeasure::well_stats(&o)?));
    }
    if !spec.wells.is_empty() {
        return Err(Error::Usage(
            "`wells` goes with stats per `well` or `field`".into(),
        ));
    }
    Ok(match spec.measure.as_str() {
        "stats" => Box::new(StatsMeasure {
            image: spec.image,
            select: spec.select.clone(),
            level: spec.level,
            per: match spec.per.as_deref().unwrap_or("channel") {
                "channel" => StatsPer::Channel,
                "image" => StatsPer::Image,
                "plane" => StatsPer::Plane,
                other => {
                    return Err(Error::Usage(format!(
                        "per `{other}`: use channel, image, plane, well or field"
                    )));
                }
            },
            mip: match spec.mip.as_deref() {
                None => None,
                Some("z" | "Z") => Some(openreadout_core::stats::Projection::Z),
                Some("t" | "T") => Some(openreadout_core::stats::Projection::T),
                Some(other) => {
                    return Err(Error::Usage(format!("mip `{other}`: use z or t")));
                }
            },
        }),
        "trace" => Box::new(TraceMeasure {
            trace: spec.trace,
            sweep: spec.sweep,
            channels: spec.channels.clone(),
            first: 0,
            count: None,
            x_range: None,
        }),
        "table" => {
            let mut o = TableOptions::default();
            o.gating_file = spec.gating_file.clone();
            o.sample = spec.sample.clone();
            o.compensation = match spec.compensate.as_deref() {
                None => None,
                Some("auto") => Some(CompensationChoice::Auto),
                Some("fcs") => Some(CompensationChoice::File),
                Some("gating") => Some(CompensationChoice::GatingFile),
                Some(other) => {
                    return Err(Error::Usage(format!(
                        "compensate `{other}`: use auto, fcs or gating"
                    )));
                }
            };
            o.transform = match spec.transform.as_deref() {
                None => None,
                Some("workspace" | "gating") => Some(TransformChoice::GatingFile),
                Some(s) => Some(TransformChoice::Uniform {
                    transform: parse_transform_spec(s)?,
                    parameters: Vec::new(),
                }),
            };
            Box::new(TableMeasure {
                table: spec.table,
                parameters: spec.parameters.clone(),
                options_text: format!(
                    "{:?}{:?}{:?}",
                    spec.compensate, spec.transform, spec.gating_file
                ),
                fcs: o,
            })
        }
        "gate" => Box::new(GateMeasure {
            gating_file: spec.gating_file.clone().ok_or_else(|| {
                Error::Usage("gate needs a FlowJo workspace or Gating-ML file".into())
            })?,
            sample: spec.sample.clone(),
            populations: spec.populations.clone(),
            medians: spec.medians.clone(),
            table: spec.table.unwrap_or(0),
        }),
        "info" => Box::new(InfoMeasure {
            fields: spec.fields.clone(),
        }),
        other => {
            return Err(Error::Usage(format!(
                "unknown measure `{other}`: stats, trace, table, gate, info, spectra, {}",
                ANALYSIS_MEASURES.join(", ")
            )));
        }
    })
}

/// The error row of a data set that holds nothing a measure applies to.
pub(crate) fn not_applicable(measure: &str, info: &FileInfo, what: &str, hint: &str) -> Error {
    Error::unsupported(
        "batch",
        format!(
            "`{measure}` needs {what}; this {} file has none",
            info.format.name
        ),
        hint,
    )
}

/// The plate well an image was acquired in, when its reader records one: `extra.well` (a name
/// such as `B03`, or `{name}` / `{row, column}` with zero-based indices) or a CZI scene's
/// `extra.scene.well`. Readers of high-content screening formats put the well there so batch
/// tables and plate layouts pick it up.
pub fn image_well(im: &ImageInfo) -> Option<String> {
    let from = |v: &serde_json::Value| -> Option<String> {
        if let Some(s) = v.as_str() {
            return crate::well::parse(s).map(crate::well::Well::name);
        }
        if let Some(s) = v.get("name").and_then(|n| n.as_str())
            && let Some(w) = crate::well::parse(s)
        {
            return Some(w.name());
        }
        let idx = |k: &[&str]| {
            k.iter()
                .find_map(|k| v.get(*k).and_then(serde_json::Value::as_u64))
        };
        let (r, c) = (
            idx(&["row", "row_index"])?,
            idx(&["column", "column_index"])?,
        );
        let w = crate::well::Well {
            row: u32::try_from(r).ok()?,
            col: u32::try_from(c).ok()?,
        };
        (w.row < crate::well::MAX_ROWS && w.col < crate::well::MAX_COLS).then(|| w.name())
    };
    im.extra.get("well").and_then(from).or_else(|| {
        im.extra
            .get("scene")
            .and_then(|s| s.get("well"))
            .and_then(from)
    })
}
