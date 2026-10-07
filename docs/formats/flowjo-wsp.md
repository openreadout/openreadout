# FlowJo workspaces (`.wsp`) and Gating-ML 2.0

FlowJo 10 saves its gating analyses as workspaces (`.wsp`). Gating-ML 2.0 is the ISAC open standard for the same gates, transforms and compensation. OpenReadout reads both into one gating model and applies it to the events of FCS files. `openreadout analyze gate` gives the gate tree and the event count of every population; `openreadout table --compensate/--transform/--population` gives compensated, transformed events with population membership; the MCP tools are `openreadout_gate` and `openreadout_table`. Derived from FlowJo 10 workspaces (FlowJo 10.6.1–10.10.0, workspace version `20.0`) and the ISAC Gating-ML 2.0 compliance files; FlowKit 1.3 and flowutils 1.2 (BSD-3) were read as prior art and serve as reference readers. Provenance: `docs/provenance/flowjo-wsp.md`.

A **Gating-ML 2.0** document (ISAC open standard) is one gating strategy: gates, transformations and compensation (`spectrumMatrix`) elements side by side under `<gating:Gating-ML>`, linked by ids. A **FlowJo workspace** is FlowJo's own XML around the same Gating-ML elements: a list of samples, each with its keywords, its compensation matrix, one transform per parameter and its complete gate tree, plus groups of samples. Both are read into one model (below): a **strategy** = populations (gate + parent) + transforms + matrices, evaluated per event on FCS scale values.

## FlowJo workspace structure (elements as they appear in the files)

| element / attribute | our field | notes |
| --- | --- | --- |
| `Workspace/@version`, `@flowJoVersion`, `@modDate` | `version`, `flowjo_version`, `modified` | `20.0`, `10.6.1`, … |
| `Groups/GroupNode/@name` → `Group/SampleRefs/SampleRef/@sampleID` | `Group` `name`, `sample_ids` | `All Samples`, `Compensation`, user groups. Group template gates (`GroupNode/Subpopulations`) are not read: every sample carries its complete tree (FlowKit makes the same choice) |
| `SampleList/Sample` | `WorkspaceSample` | one per FCS file |
| `Sample/DataSet/@uri`, `@sampleID` | `uri`, `id` | `file:/V:/…/name.fcs`, percent-encoded (`%20`) |
| `Sample/Keywords/Keyword/@name,@value` | `keywords` | the FCS TEXT keywords plus FlowJo's own (`FJ_FCS_VERSION`, …) |
| `Sample/SampleNode/@name`, `@count` | `name`, `event_count` | the sample name is usually the FCS file name |
| `Sample/transforms:spilloverMatrix` | `matrix` | `@name` (`Acquisition-defined`), `@prefix` (`Comp-`), `@suffix`, `@spectral` (`1` = spectral), `@weightOptAlgorithmType` (`OLS`); `data-type:parameters/data-type:parameter/@data-type:name` (detectors); one `transforms:spillover` row per fluorochrome (`@data-type:parameter`) with `transforms:coefficient/@data-type:parameter,@transforms:value` |
| `Sample/Transformations/*` | `transforms` (keyed by parameter) | one element per parameter, the parameter in `data-type:parameter/@data-type:name` (see Transforms) |
| `SampleNode/Subpopulations/Population/@name,@count,@owningGroup` | `Population` `name`, `stored_count`, `owning_group` | `count="-1"`: not computed. `owningGroup=""`: a gate specific to this sample |
| `Population/Gate/gating:*Gate/@eventsInside` | `complement` | `0`: the population is the events **outside** the gate |
| `Subpopulations/AndNode`, `OrNode`, `NotNode` + `Dependents/Dependent/@name` | Boolean `Shape` | dependents are paths from the root without it, `/`-separated (`Time/Singlets/aAmine-/CD3+/CD4+/IFNg+`) |

Gate dimensions name parameters (`data-type:fcs-dimension/@data-type:name`). A name with the sample matrix's prefix and suffix (`Comp-TNFa FITC FLR-A`) is that parameter **compensated** by the sample's matrix; without them it is uncompensated. FlowJo stores gate coordinates as **scale values** (before the transform); gates are evaluated in the **transformed** space, so the evaluator moves bounds and vertices through the parameter's transform first (for monotone transforms of rectangles this changes nothing; for polygons it does). FlowJo spells a `$PnN` containing `/` with `_`.

### FlowJo ellipses

`gating:EllipsoidGate` in a workspace holds two `gating:foci` vertices and four `gating:edge` vertices in FlowJo's 256-bin display space of the transformed parameters, and a `gating:distance`. We follow FlowKit's reading: coordinates / 256 give the transformed value (× 4096 for FlowJo biex, whose table output is 0–4096), the half-axis `a` is the larger rotated edge offset, `b = sqrt(|f² − a²|)` with `f` the rotated focus offset, and the ellipse is evaluated as the 128-vertex polygon through its boundary (FlowJo also turns ellipses into polygons). In `flowkit-line-ellipse` this reproduces FlowJo's stored count (51 of 100).

### Quadrant gates in workspaces

FlowJo 10 writes a quadrant gate as four `Population`s, each a `gating:RectangleGate` with one open side (`flowkit-diamond-quad`, names `Q1: channel_A- , channel_B+` …). They are read as rectangles.

## Gating-ML 2.0 (elements under `gating:Gating-ML`)

| element | our model |
| --- | --- |
| `gating:RectangleGate` | `GateKind::Rectangle`: each `gating:dimension` with optional `gating:min` (inclusive) and `gating:max` (exclusive) |
| `gating:PolygonGate` | `GateKind::Polygon`: two dimensions, `gating:vertex/gating:coordinate/@data-type:value` |
| `gating:EllipsoidGate` | `GateKind::Ellipsoid`: `gating:mean`, `gating:covarianceMatrix/gating:row/gating:entry`, `gating:distanceSquare`; inside when `(p − mean)ᵀ C⁻¹ (p − mean) <= distanceSquare` (any number of dimensions) |
| `gating:QuadrantGate` | `gating:divider` (`gating:id`, a dimension, `gating:value`s) and `gating:Quadrant/gating:position/@gating:divider_ref,@gating:location`; each quadrant becomes a `GateKind::Quadrant` population whose bounds on each divider are the largest value `<= location` and the smallest `> location` |
| `gating:BooleanGate` | `gating:and`/`gating:or`/`gating:not` of `gating:gateReference/@gating:ref` with `@gating:use-as-complement` |
| `@gating:parent_id` | parent; a quadrant id can be a parent, a quadrant gate id cannot |
| `gating:dimension/@gating:compensation-ref` | `uncompensated`, `FCS` (the file's own `$SPILLOVER`/`$SPILL`/`SPILL`; uncompensated when the file has none) or a `spectrumMatrix` id |
| `gating:dimension/@gating:transformation-ref` | a `transforms:transformation` id |
| `data-type:new-dimension/@data-type:transformation-ref` | a ratio dimension (`fratio`) |
| `transforms:spectrumMatrix` | `transforms:fluorochromes` (rows) × `transforms:detectors` (columns), `transforms:spectrum/transforms:coefficient/@transforms:value`; `@matrix-inverted-already="true"` is inverted back. A dimension may name a fluorochrome; its values are those of the matching detector after compensation |

Paths: `/` + ids from the root; a quadrant's path includes its quadrant gate (`/Quadrant1/FL2P-FL4P`). Gate ids must be unique; cycles through parents or Boolean references are rejected (corrupt file, exit 4).

## Transforms

Gating-ML definitions (T top of scale, M decades, W linear width in decades, A extra negative decades):

| name (`kind`) | element | value |
| --- | --- | --- |
| `linear` | `transforms:flin` (T, A); FlowJo `linear` (T = `maxRange`, A = `minRange`, as FlowKit reads it) | `(x + A) / (T + A)` |
| `log` | `transforms:flog` (T, M) | `log10(x / T) / M + 1`; not a number for `x <= 0` |
| `arcsinh` | `transforms:fasinh` (T, M, A); FlowJo `fasinh` (its extra `length`, `maxRange`, `W` are ignored) | `(asinh(x · sinh(M ln10) / T) + A ln10) / ((M + A) ln10)` |
| `logicle` | `transforms:logicle` (T, W, M, A); FlowJo `logicle` | root of the biexponential (Moore & Parks 2012); T maps to 1, 0 to `W/(M+A)` |
| `hyperlog` | `transforms:hyperlog` (T, W, M, A) | root of `a·e^(by) + c·y − f` |
| `ratio` | `transforms:fratio` (A, B, C) over two parameters | `A (x − B) / (y − C)`: a new dimension |
| `flowjo-log` | FlowJo `log` (`offset`, `decades`) | `(log10(max(x, offset)) − log10(offset)) / decades` |
| `flowjo-biex` | FlowJo `biex` (`neg`, `width`, `pos`, `maxRange`) | 4097-point table (FlowJo's construction as ported by FlowKit), linear interpolation, clamped to 0–4096 |
| `arcsinh-cofactor` | command line only | `asinh(x / cofactor)` (mass cytometry: 5) |

Parameters are checked before use (logicle needs `0 <= W <= M/2` and `−W <= A <= M − 2W`); bad values are unsupported errors (exit 6) with a hint. FlowJo transform elements not in the table are kept as `unsupported_transforms` and reported only when a gate uses them.

## Compensation

`S` = spillover matrix (rows fluorochromes, columns detectors). Square: `compensated = raw · S⁻¹` (Gauss–Jordan with partial pivoting). Spectral (more detectors than rows; FlowJo `spectral="1"` with `OLS`): ordinary least squares, `unmixed = raw · Sᵀ (S Sᵀ)⁻¹`, written into the columns named by the rows. Singular matrices are rejected. The file's own matrix is `$SPILLOVER`, `$SPILL` or `SPILL` (the latter two only when every name is a `$PnN`); `$COMP` is not used for compensation.

## Evaluation

1. Raw DATA values → **scale values** (FCS 3.1 §3.2.19–20): `$PnE f1,f2` with `f1 > 0` → `10^(f1·x/$PnR)·f2` (`f2 = 0` read as 1); `$PnG` ≠ 0, 1 → `x / gain`; the `Time` parameter × `$TIMESTEP` (its gain ignored). This is what FlowIO/FlowKit call preprocessing, and what Gating-ML gates are written against (`flowkit-gml-events`: `$P1G/3.67/`, `$P3E/4,1/`).
2. Per gate dimension: compensation (per reference, computed once per block of events), then the transform.
3. Rectangle `[min, max)`; polygon by the winding number (inside when odd; points outside the bounding box are outside); ellipsoid by the Mahalanobis distance; FlowJo ellipse as its 128-gon.
4. `eventsInside="0"` complements; the parent's membership is intersected; Boolean gates combine other populations' memberships (`not` takes one operand).

Every step is per event, so files are streamed in blocks of 65,536 events.

## Validation (corpus harness `tests/flow.rs`, oracle `oracle/flow.py`)

- **Population counts** equal FlowKit's exactly: 228 populations over 17 workspace/sample pairs (the Gating-ML compliance file on `data1.fcs`, 49 populations; the FlowKit line and diamond workspaces; the 8-colour ICS workspaces with logicle transforms, Comp- dimensions, an ellipse, Boolean And/Or/Not nodes and a reused quadrant gate on three 283,969–290,172-event files; a FlowJo 10.10 spectral workspace from a BD FACSDiscover S8 — OLS unmixing of 78 detectors, biex and log transforms — on an FCS 3.2 file).
- **FlowJo's own stored counts** (read from the workspace independently of both readers): 55 of 177 identical; median difference 0.17 %, at most 3.2 % for populations of 1,000 events or more, the largest absolute difference 704 events on `/Time/Singlets` (235,331 vs 236,035). Small cytokine gates differ by a few events. FlowKit shows the same differences; FlowJo evaluates on its own binned display values.
- **Compensation**: 9 files (BD FACSDiva `SPILL`, CytoFLEX and Aurora `$SPILLOVER`, FlowKit's `$SPILL` example, a workspace matrix, and a 7 × 81 spectral OLS workspace matrix), 1,000 sampled events per output column and the sum of every column over all events: worst error 5.8 × 10⁻¹³ for square matrices and 4.3 × 10⁻¹¹ for the spectral one (relative, absolute below 1; FlowKit solves the least-squares problem with NumPy's `lstsq`, we with the normal equations); column sums 2.6 × 10⁻¹⁸ per event. **Transforms** (logicle ×2, arcsinh ×2, hyperlog, log, linear, FlowJo biex ×2, FlowJo log, arcsinh-cofactor ×2) of those compensated values: worst 4.0 × 10⁻¹². The harness tolerance is 10⁻⁹.

## Not supported (yet)

FlowJo derived parameters and scripts, statistics nodes, layouts and table editor content; transforms other than those above (reported as unsupported when a gate uses them); spectral unmixing other than OLS; group template gates without a sample; `.acs` archives. FlowRepository workspaces were not added: its TLS certificate still failed verification on 2026-09-23 and per-dataset licences are not uniformly stated.

## Vocabulary (every public identifier in `openreadout-fcs/src/gating/`, and the analysis entry points)

| identifier | meaning |
| --- | --- |
| `read_gating_file`, `GatingFile` { `Workspace`, `GatingMl` }, `WSP_FORMAT_ID`, `GATING_ML_FORMAT_ID` | open a `.wsp` or Gating-ML file (by root element); format ids `flowjo-wsp`, `gating-ml` |
| `Workspace`, `version`, `flowjo_version`, `modified`, `samples`, `groups`, `select_sample`, `parse_workspace` | a FlowJo workspace; choosing the sample for an FCS file (by `--sample`, then the file name or the `DataSet/@uri` file name, then `$FIL`, then the only sample) |
| `select_sample_by_keywords`, `ACQUISITION_KEYWORDS` | last resort for a renamed FCS file: the one sample whose stored keywords `$TOT`, `$BTIM`, `$ETIM`, `$DATE`, `$CYT`, `$CYTSN` all agree with the file's (at least three compared, `$TOT` among them) |
| `Group`, `name`, `sample_ids` | a sample group |
| `WorkspaceSample`, `id`, `name`, `uri`, `uri_file_name`, `keywords`, `keyword`, `event_count`, `matrix`, `matrix_prefix`, `matrix_suffix`, `strategy`, `skipped`, `groups` | one workspace sample |
| `Strategy`, `populations`, `transforms`, `matrices`, `unsupported_transforms`, `find`, `subtree` | a gating strategy |
| `Population`, `name`, `path`, `path_string`, `parent`, `kind`, `dimensions`, `shape`, `complement`, `coordinates`, `gate_id`, `quadrant_gate`, `stored_count`, `owning_group` | one population |
| `GateKind` { `Rectangle`, `Polygon`, `Ellipsoid`, `Quadrant`, `Boolean` }, `name` | gate kinds |
| `Shape` { `Rectangle`, `Polygon`, `Ellipsoid`, `DisplayEllipse`, `Boolean` }, `vertices`, `mean`, `covariance`, `distance_square`, `foci`, `edge`, `op`, `operands` | gate geometry as stored |
| `BoolOp` { `And`, `Or`, `Not` }, `Operand`, `population`, `complement` | Boolean gates |
| `Dimension`, `source`, `compensation`, `transform`, `min`, `max`, `name`, `DimensionSource` { `Parameter`, `Ratio` } | a gate axis |
| `CompensationRef` { `Uncompensated`, `FromFile`, `Matrix` }, `label` | which compensation a dimension uses |
| `CoordinateSpace` { `Transformed`, `Untransformed` } | where stored coordinates live |
| `CompMatrix`, `name`, `detectors`, `fluorochromes`, `values`, `spectral`, `from_spillover`, `solver`, `Solver`, `apply_row`, `apply_columns`, `invert` | compensation matrices and applying them |
| `Transform` { `Linear`, `Log`, `Asinh`, `Logicle`, `Hyperlog`, `Ratio`, `FlowJoLog`, `FlowJoBiex`, `AsinhCofactor` }, `t`, `w`, `m`, `a`, `b`, `c`, `numerator`, `denominator`, `offset`, `decades`, `negative`, `width`, `positive`, `max_value`, `cofactor`, `kind`, `parameters`, `prepare` | transforms and their parameters |
| `Prepared`, `apply`, `ratio`, `display_range`, `Biexponential`, `logicle_scale`, `logicle_inverse`, `hyperlog_scale`, `BiexTable` | transforms ready to apply |
| `Evaluator`, `new`, `evaluate`, `compensations_used`, `PopulationCount`, `population`, `count` | binding a strategy to a file and evaluating blocks of events |
| `parse_gating_ml` | Gating-ML parser |
| `read_text`, `root_local_name`, `parse`, `attr`, `child`, `children`, `elements`, `num_attr`, `line` | namespace-tolerant XML helpers |
| `GateRequest`, `gating_file`, `fcs`, `sample`, `table`, `populations`, `gate` | `openreadout analyze gate` |
| `medians` (request: parameters; `PopulationRow.medians`: name → median), `median` | `analyze gate --median PARAM`: each population's median of a parameter's scale values (`Comp-PARAM`: compensated with the sample's matrix, else the gating file's only matrix, else the file's spillover); the middle value, or the mean of the two middle values |
| `TableOptions`, `compensation`, `transform`, `gating_file`, `sample`, `populations`, `is_active`, `CompensationChoice` { `Auto`, `File`, `GatingFile` }, `TransformChoice` { `Uniform`, `GatingFile` }, `parameters`, `parse_transform_spec` | `table` processing options |
| `ProcessedRows`, `names`, `labels`, `columns`, `total_rows`, `processing`, `processed_rows`, `table_slice` | processed table rows |
