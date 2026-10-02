//! `gate --tidy`: gated populations of every FCS file, one row per file × population, with
//! counts, percentages, the count the gating software stored and, for `--median` parameters,
//! the population's median (`median:<parameter>`).

use std::path::PathBuf;

use openreadout_core::Result;
use openreadout_fcs::analysis::{GateRequest, gate};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};

/// Population statistics from a FlowJo workspace or Gating-ML file.
#[derive(Debug, Clone, Default)]
pub struct GateMeasure {
    /// The FlowJo workspace (`.wsp`) or Gating-ML document.
    pub gating_file: PathBuf,
    /// Workspace sample (default: matched per FCS file by name or `$FIL`).
    pub sample: Option<String>,
    /// Only these populations and their descendants.
    pub populations: Vec<String>,
    /// Parameters whose population medians are reported.
    pub medians: Vec<String>,
    /// FCS data set index.
    pub table: u32,
}

impl Measure for GateMeasure {
    fn id(&self) -> &'static str {
        "gate"
    }
    fn grain(&self) -> Vec<String> {
        vec!["population".into()]
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        vec![
            ColumnDoc::key("population", "population path (/Lymphocytes/Singlets/CD3+)"),
            ColumnDoc::key("name", "population name"),
            ColumnDoc::value("parent", "parent population path"),
            ColumnDoc::value(
                "gate_type",
                "rectangle, polygon, ellipsoid, quadrant, boolean",
            ),
            ColumnDoc::meta(
                "workspace_sample",
                "the workspace sample whose gates were used",
            ),
            ColumnDoc::value("events_total", "events in the FCS data set"),
            ColumnDoc::value("count", "events in the population"),
            ColumnDoc::unit("percent_of_parent", "%", "count / parent count × 100"),
            ColumnDoc::unit("percent_of_total", "%", "count / all events × 100"),
            ColumnDoc::value("stored_count", "the count FlowJo stored in the workspace"),
            ColumnDoc::value(
                "median:*",
                "median of the parameter over the population's events (scale values; Comp- = compensated)",
            ),
        ]
    }
    fn fingerprint(&self) -> String {
        format!(
            "gate:{}:{:?}:{:?}:{:?}:{}",
            self.gating_file.display(),
            self.sample,
            self.populations,
            self.medians,
            self.table
        )
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        if it.info.format.id != openreadout_fcs::FORMAT_ID {
            return Err(super::not_applicable(
                "gate",
                it.info,
                "FCS events",
                "Gating applies to FCS files; filter the inputs with --format fcs.",
            ));
        }
        let mut req = GateRequest::new(self.gating_file.clone(), Some(it.path.to_path_buf()));
        req.sample = self.sample.clone();
        req.populations = self.populations.clone();
        req.medians = self.medians.clone();
        req.table = self.table;
        let out = gate(&req)?;
        let sample = out.sample.as_ref().map(|s| s.name.clone());
        let mut rows = Vec::new();
        for p in &out.populations {
            let mut r = Row::new()
                .with("population", p.path.as_str())
                .with("name", p.name.as_str())
                .with("parent", Value::text_opt(p.parent.as_deref()))
                .with("gate_type", p.gate_type.as_str());
            r.set("workspace_sample", Value::text_opt(sample.as_deref()));
            r.set("events_total", out.event_count);
            r.set("count", p.count);
            r.set("percent_of_parent", Value::float_opt(p.percent_of_parent));
            r.set("percent_of_total", Value::float_opt(p.percent_of_total));
            r.set("stored_count", p.stored_count);
            for m in self
                .medians
                .iter()
                .flat_map(|m| m.split(','))
                .map(str::trim)
            {
                if m.is_empty() {
                    continue;
                }
                r.set(
                    format!("median:{m}"),
                    Value::float_opt(p.medians.get(m).copied()),
                );
            }
            rows.push(r);
        }
        Ok(rows)
    }
}
