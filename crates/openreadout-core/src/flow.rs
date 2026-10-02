//! JSON shapes of flow-cytometry analysis: `openreadout analyze gate` / `openreadout_analyze` kind
//! `gate` (FlowJo workspace or Gating-ML populations with event counts) and the processing record
//! that `table --compensate/--transform/--gatingml/--workspace` attaches to a table slice. The
//! computation lives in the FCS reader crate; these types are shared by the CLI and MCP.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Output of `analyze gate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GateOutput {
    /// The FCS file gated; absent when only the gating file was described.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The gating file (FlowJo workspace or Gating-ML document).
    pub gating_file: String,
    /// `flowjo-wsp` or `gating-ml`.
    pub gating_format: String,
    /// Workspace-level facts (FlowJo only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceSummary>,
    /// The workspace sample whose gates were used (FlowJo only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<SampleSummary>,
    /// FCS data set (table) gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<u32>,
    /// Events in the data set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_count: Option<u64>,
    /// Compensation matrices the gating file defines (and whether any gate used them).
    pub compensation: Vec<CompensationSummary>,
    /// Transforms by id (FlowJo: by parameter name).
    pub transforms: Vec<TransformSummary>,
    /// One row per population, parents before children.
    pub populations: Vec<PopulationRow>,
    /// The same populations as a tree (quadrant gates appear as grouping nodes without a count).
    pub tree: Vec<PopulationNode>,
    /// Caveats: skipped populations, disagreements with counts stored in the workspace, scaling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Facts about a FlowJo workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceSummary {
    /// Workspace format version (`20.0`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// FlowJo release that saved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flowjo_version: Option<String>,
    /// Modification date as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// Groups and their member sample names.
    pub groups: Vec<GroupSummary>,
    /// Every sample in the workspace.
    pub samples: Vec<SampleSummary>,
}

/// A workspace group.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GroupSummary {
    /// Group name.
    pub name: String,
    /// Sample names in the group.
    pub samples: Vec<String>,
}

/// A workspace sample.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SampleSummary {
    /// FlowJo sample id.
    pub id: String,
    /// Sample name.
    pub name: String,
    /// Where FlowJo found the FCS file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Event count FlowJo stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_count: Option<u64>,
    /// Groups the sample belongs to.
    pub groups: Vec<String>,
    /// Populations in the sample's gate tree.
    pub population_count: usize,
    /// Name of the sample's compensation matrix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation: Option<String>,
    /// Keywords FlowJo stored with the sample (FCS TEXT plus its own).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keywords: BTreeMap<String, String>,
}

/// A compensation or unmixing matrix.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CompensationSummary {
    /// Matrix name or id; `$SPILLOVER`/`$SPILL`/`SPILL` for the FCS file's own.
    pub name: String,
    /// Where it comes from: `workspace`, `gating-ml`, `fcs` or `command-line`.
    pub source: String,
    /// Detectors (columns), `$PnN`.
    pub detectors: Vec<String>,
    /// Rows: fluorochromes, or the detectors that receive the unmixed values.
    pub fluorochromes: Vec<String>,
    /// Row-major coefficients (spillover fractions).
    pub matrix: Vec<Vec<f64>>,
    /// More detectors than rows: unmixed by ordinary least squares.
    pub spectral: bool,
    /// Whether a gate (or the table) used it.
    pub used: bool,
}

/// A parameter transform defined in the gating file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TransformSummary {
    /// Id (FlowJo: the parameter it applies to).
    pub id: String,
    /// `linear`, `log`, `arcsinh`, `logicle`, `hyperlog`, `ratio`, `flowjo-log`, `flowjo-biex`,
    /// `arcsinh-cofactor`, or the element name of an unsupported FlowJo transform.
    pub kind: String,
    /// Parameters (`T`, `W`, `M`, `A`, `offset`, `decades`, `neg`, `width`, `pos`, `maxRange`, …).
    pub parameters: BTreeMap<String, f64>,
    /// False for transforms this reader does not evaluate.
    pub supported: bool,
}

/// One gate axis.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GateDimension {
    /// Parameter (`$PnN`, fluorochrome) or ratio id.
    pub parameter: String,
    /// `uncompensated`, `FCS` or a matrix name.
    pub compensation: String,
    /// Transform id, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
    /// Lower bound (inclusive), as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Upper bound (exclusive), as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// One population.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PopulationRow {
    /// `/`-separated path from the root (`/Lymphocytes/Single Cells/CD3+`).
    pub path: String,
    /// Population name.
    pub name: String,
    /// Parent path; absent for populations of all events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// `rectangle`, `polygon`, `ellipsoid`, `quadrant` or `boolean`.
    pub gate_type: String,
    /// Gate axes.
    pub dimensions: Vec<GateDimension>,
    /// Gate geometry as stored: `vertices`, `mean`/`covariance`/`distance_square`, `foci`/`edge`,
    /// or `op`/`operands` (paths, with `complement`).
    pub gate: serde_json::Value,
    /// Events outside the gate form the population (FlowJo `eventsInside="0"`).
    pub complement: bool,
    /// Coordinates are stored before the transform (`untransformed`, FlowJo) or after it.
    pub coordinates: String,
    /// Events in the population (absent when no FCS file was given).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Percent of the parent population's events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_of_parent: Option<f64>,
    /// Percent of all events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_of_total: Option<f64>,
    /// The count the gating software stored (FlowJo `count`), for comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_count: Option<u64>,
    /// Median fluorescence (or scatter) of the population's events for each parameter asked for
    /// (`analyze gate --median`), keyed by the name as asked: scale values (after `$PnE`/`$PnG`),
    /// compensated when the name carries the workspace's `Comp-` prefix. Absent for an empty
    /// population.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub medians: BTreeMap<String, f64>,
}

/// A node of the population tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PopulationNode {
    /// Population name.
    pub name: String,
    /// `/`-separated path from the root (`/Lymphocytes/Single Cells/CD3+`).
    pub path: String,
    /// Gate type, or `quadrant-gate` for the node grouping a quadrant gate's quadrants.
    pub gate_type: String,
    /// Events (absent without an FCS file, and for quadrant-gate grouping nodes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Child populations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<PopulationNode>,
}

/// What `table` did to the raw values before returning them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TableProcessing {
    /// Channel-to-scale conversions applied (`$PnE`, `$PnG`, `$TIMESTEP`); every processed table
    /// starts from scale values.
    pub scale: Vec<String>,
    /// The compensation applied, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation: Option<CompensationSummary>,
    /// Transforms applied, by parameter.
    pub transforms: Vec<AppliedTransform>,
    /// Membership columns appended (1 = in the population, 0 = not).
    pub gates: Vec<GateColumn>,
}

/// A transform applied to one table column.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AppliedTransform {
    /// Column (`$PnN`).
    pub parameter: String,
    /// Transform kind.
    pub kind: String,
    /// Its parameters.
    pub parameters: BTreeMap<String, f64>,
}

/// A population membership column.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GateColumn {
    /// Column name in the table (`gate:<path>`).
    pub column: String,
    /// Population path.
    pub population: String,
    /// Path of the workspace or Gating-ML file that defines the population.
    pub gating_file: String,
    /// Members among the returned rows.
    pub count_in_rows: u64,
}
