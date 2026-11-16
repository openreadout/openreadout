//! Slices, projections and reduced views of a stored 2D (or 3D) spectrum, read row by row
//! through [`crate::nmr::source2d`], so none of them holds the whole matrix in memory.
//!
//! - [`SliceSpec`]: a row (the F2 trace at an F1 shift), a column (the F1 trace at an F2 shift)
//!   or a projection onto F1 or F2: `skyline` (the largest value along the collapsed dimension,
//!   the usual NMR projection), `min` (the smallest: negative peaks such as the CH₂ of an
//!   edited HSQC) or `sum`. [`read_slice`] computes one.
//! - [`NmrMatrixDataset`]: wraps a data set so that those slices read as extra traces (and the
//!   matrix as an extra long-form table, `f1`, `f2`, intensity), so `trace`, `export` and
//!   `plot` take them like any stored 1-D spectrum.
//! - [`read_pooled`]: the matrix reduced to a grid of cells for drawing. Each cell keeps the
//!   largest and the smallest value of the points it covers (max and min pooling), so a narrow
//!   peak survives any reduction; the full-resolution skyline projections and a noise estimate
//!   come from the same pass.
//! - [`dimension_axes`]: what each axis measures. Most are chemical shifts in ppm; the F1 axis
//!   of a J-resolved spectrum is a coupling in Hz, and the F1 axis of a DOSY spectrum carries
//!   the unit the file names (Bruker `AXUNIT`, e.g. `log(m2/s)`).
//!
//! Projections and pooled views of a 3D spectrum collapse the slowest dimension as well (onto
//! the F2–F3 plane); rows and columns need a 2D spectrum.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, LsEntry, SignalChannelInfo, Table, TableInfo, Trace,
    TraceInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::{Dataset, Error, Plane, PlaneIndex, Result};

use crate::nmr::peaks2d::matrix_noise_sd;
use crate::nmr::process::PpmAxis;
use crate::nmr::source::is_fid;
use crate::nmr::source2d::{
    DEFAULT_MAX_POINTS, MatrixLayout, choose_matrix_trace, matrix_layout, normalize_nucleus,
};

/// How a projection combines the points along the dimension it collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionMethod {
    /// The largest value (the skyline projection NMR software draws).
    Skyline,
    /// The smallest value (negative peaks).
    Min,
    /// The sum.
    Sum,
}

impl ProjectionMethod {
    fn name(self) -> &'static str {
        match self {
            ProjectionMethod::Skyline => "skyline",
            ProjectionMethod::Min => "min",
            ProjectionMethod::Sum => "sum",
        }
    }
}

/// A 1-D trace cut out of a 2D spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliceSpec {
    /// The F2 trace (a row) at the F1 point nearest this shift, ppm.
    Row(f64),
    /// The F1 trace (a column) at the F2 point nearest this shift, ppm.
    Column(f64),
    /// The projection onto dimension `onto` (0 = F1, 1 = F2 of the plane).
    Projection {
        /// The dimension kept.
        onto: usize,
        /// How the other one is collapsed.
        method: ProjectionMethod,
    },
}

/// The forms [`SliceSpec::parse`] accepts, for error messages.
pub const SLICE_FORMS: &str = "row:PPM (the F2 trace at an F1 shift), column:PPM (the F1 trace at an F2 shift), skyline:f1|f2, min:f1|f2 or sum:f1|f2 (projections)";

impl SliceSpec {
    /// Parse `row:4.12`, `column:7.26`, `skyline:f2`, `min:f1` or `sum:f2`.
    pub fn parse(s: &str) -> Result<SliceSpec> {
        let err = || Error::Usage(format!("slice {s:?}: expected {SLICE_FORMS}"));
        let (kind, arg) = s.trim().split_once(':').ok_or_else(err)?;
        let kind = kind.trim().to_ascii_lowercase();
        let arg = arg.trim();
        let ppm = || {
            arg.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(err)
        };
        let onto = || match arg.to_ascii_lowercase().as_str() {
            "f1" => Ok(0),
            "f2" => Ok(1),
            _ => Err(err()),
        };
        Ok(match kind.as_str() {
            "row" => SliceSpec::Row(ppm()?),
            "column" | "col" => SliceSpec::Column(ppm()?),
            "skyline" | "max" => SliceSpec::Projection {
                onto: onto()?,
                method: ProjectionMethod::Skyline,
            },
            "min" => SliceSpec::Projection {
                onto: onto()?,
                method: ProjectionMethod::Min,
            },
            "sum" => SliceSpec::Projection {
                onto: onto()?,
                method: ProjectionMethod::Sum,
            },
            _ => return Err(err()),
        })
    }

    /// The dimension of the plane the result runs along (0 = F1, 1 = F2).
    pub fn along(&self) -> usize {
        match *self {
            SliceSpec::Row(_) => 1,
            SliceSpec::Column(_) => 0,
            SliceSpec::Projection { onto, .. } => onto,
        }
    }

    /// The text form ([`SliceSpec::parse`] reads it back).
    pub fn text(&self) -> String {
        match *self {
            SliceSpec::Row(p) => format!("row:{p}"),
            SliceSpec::Column(p) => format!("column:{p}"),
            SliceSpec::Projection { onto, method } => format!("{}:f{}", method.name(), onto + 1),
        }
    }
}

/// What one axis of a 2D spectrum measures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DimensionAxis {
    /// `chemical_shift`, `coupling` (the F1 axis of a J-resolved spectrum) or `other` (a DOSY
    /// diffusion axis, or any axis whose unit the file names).
    pub quantity: String,
    /// Unit of the values shown: `ppm`, `Hz`, or the file's own unit text.
    pub unit: String,
    /// Nucleus, when the file names one (`1H`, `13C`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
    /// Shown value = `scale` × the axis value in ppm (1 for ppm and for file units; the
    /// spectrometer frequency in MHz for Hz).
    pub scale: f64,
}

impl DimensionAxis {
    /// The value shown for axis position `ppm`.
    pub fn show(&self, ppm: f64) -> f64 {
        ppm * self.scale
    }
    /// The axis position (ppm) of shown value `v`.
    pub fn unshow(&self, v: f64) -> f64 {
        if self.scale == 0.0 { v } else { v / self.scale }
    }
    /// Is this a chemical shift in ppm?
    pub fn is_shift(&self) -> bool {
        self.quantity == "chemical_shift"
    }
    /// A short slug for column names: `ppm`, `hz`, or the unit with non-alphanumerics dropped.
    pub fn slug(&self) -> String {
        let s: String = self
            .unit
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() { "value".into() } else { s }
    }
}

fn text<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// The pulse program of the data set's FID, when the reader reports one (Bruker `PULPROG`).
pub fn pulse_program(info: &FileInfo) -> Option<String> {
    info.traces
        .iter()
        .find_map(|t| {
            let key = if is_fid(t) {
                "pulse_program"
            } else {
                "pulse_program_of_fid"
            };
            t.extra.get(key).and_then(Value::as_str)
        })
        .map(str::to_string)
}

/// What each dimension of `t` (with layout `lay`) measures, slowest first (module docs).
pub fn dimension_axes(
    info: &FileInfo,
    t: &TraceInfo,
    vendor: &Value,
    lay: &MatrixLayout,
) -> Vec<DimensionAxis> {
    let dims = lay.shape.len();
    let procno = t
        .extra
        .get("procno")
        .and_then(Value::as_u64)
        .map(|p| p.to_string());
    let program = pulse_program(info).unwrap_or_default().to_ascii_lowercase();
    let direct = &lay.axes[dims - 1];
    let direct_span_hz =
        (direct.step_ppm * direct.size as f64 * direct.reference_frequency_mhz).abs();
    (0..dims)
        .map(|d| {
            let a = &lay.axes[d];
            let nucleus = lay.nuclei.get(d).cloned().flatten();
            let shift = DimensionAxis {
                quantity: "chemical_shift".into(),
                unit: "ppm".into(),
                nucleus: nucleus.clone(),
                scale: 1.0,
            };
            if d + 1 == dims {
                return shift;
            }
            // Bruker: the unit the processing parameters give this dimension
            let unit = procno.as_ref().and_then(|p| {
                let file = format!("proc{}s", dims - d);
                text(vendor, &format!("/pdata/{p}/{file}/parameters/AXUNIT"))
                    .map(|u| u.trim().trim_matches(['<', '>']).trim().to_string())
            });
            if let Some(u) = unit.filter(|u| {
                !u.is_empty() && !u.eq_ignore_ascii_case("ppm") && !u.eq_ignore_ascii_case("hz")
            }) {
                return DimensionAxis {
                    quantity: "other".into(),
                    unit: u,
                    nucleus: None,
                    scale: 1.0,
                };
            }
            // J-resolved: the same nucleus over a few tens of Hz around 0
            let span_hz = (a.step_ppm * a.size as f64 * a.reference_frequency_mhz).abs();
            let same = nucleus.is_some()
                && nucleus.as_deref().and_then(normalize_nucleus)
                    == lay.nuclei[dims - 1].as_deref().and_then(normalize_nucleus);
            let jres = program.contains("jres")
                || (same && span_hz > 0.0 && span_hz < 500.0 && direct_span_hz > 10.0 * span_hz);
            if jres && a.reference_frequency_mhz > 0.0 {
                return DimensionAxis {
                    quantity: "coupling".into(),
                    unit: "Hz".into(),
                    nucleus,
                    scale: a.reference_frequency_mhz,
                };
            }
            shift
        })
        .collect()
}

/// A matrix trace of `info`: its index, layout and per-dimension description.
pub(crate) struct MatrixTrace {
    pub index: u32,
    pub name: Option<String>,
    pub layout: MatrixLayout,
    pub axes: Vec<DimensionAxis>,
}

pub(crate) fn matrix_trace(
    info: &FileInfo,
    vendor: &Value,
    requested: Option<u32>,
) -> Result<MatrixTrace> {
    let index = choose_matrix_trace(info, vendor, requested)?;
    let t = info
        .traces
        .iter()
        .find(|t| t.index == index)
        .ok_or_else(|| Error::Usage(format!("trace {index} out of range")))?;
    let layout = matrix_layout(t, vendor).ok_or_else(|| {
        Error::unsupported(
            "nmr-processing",
            format!("trace {index} has no matrix layout"),
            "Pick a stored 2D/3D spectrum (see `openreadout info`).",
        )
    })?;
    let axes = dimension_axes(info, t, vendor, &layout);
    Ok(MatrixTrace {
        index,
        name: t.name.clone(),
        layout,
        axes,
    })
}

/// Index window `[lo, hi]` of `axis` for a ppm range (either order), clamped; an error when
/// the range misses the axis.
fn window(axis: &PpmAxis, range: Option<(f64, f64)>, dim: usize) -> Result<(usize, usize)> {
    let n = axis.size;
    if n == 0 {
        return Err(Error::corrupt(
            "nmr-processing",
            "a dimension has no points",
        ));
    }
    let Some((a, b)) = range else {
        return Ok((0, n - 1));
    };
    let (i, j) = (axis.index_of(a), axis.index_of(b));
    let (i, j) = if i <= j { (i, j) } else { (j, i) };
    if j < -0.5 || i > n as f64 - 0.5 {
        return Err(Error::Usage(format!(
            "range {a}..{b} is outside F{} ({:.4}..{:.4} ppm)",
            dim + 1,
            axis.first_ppm,
            axis.last_ppm()
        )));
    }
    let lo = i.round().clamp(0.0, (n - 1) as f64) as usize;
    let hi = j.round().clamp(0.0, (n - 1) as f64) as usize;
    Ok((lo, hi.max(lo)))
}

/// Calls `f(plane_row, values)` for every matrix row whose F1 index (the second-slowest
/// dimension, the row of the plane) is in `rows`, reading columns `cols` of it. 3D spectra:
/// every plane along the slowest dimension is visited, in order.
fn for_rows(
    ds: &mut dyn Dataset,
    m: &MatrixTrace,
    rows: (usize, usize),
    cols: (usize, usize),
    max_points: usize,
    f: &mut dyn FnMut(usize, &[f64]) -> Result<()>,
) -> Result<()> {
    let lay = &m.layout;
    let dims = lay.shape.len();
    let outer: usize = lay.shape[..dims - 2].iter().product();
    let n1 = lay.shape[dims - 2];
    let ncol = cols.1 - cols.0 + 1;
    let total = outer
        .checked_mul(rows.1 - rows.0 + 1)
        .and_then(|r| r.checked_mul(ncol))
        .unwrap_or(usize::MAX);
    if total > max_points {
        return Err(too_many(total, max_points));
    }
    for o in 0..outer {
        for r in rows.0..=rows.1 {
            let row = o
                .checked_mul(n1)
                .and_then(|x| x.checked_add(r))
                .ok_or_else(|| Error::corrupt("nmr-processing", "row index overflows"))?;
            let sweep = u32::try_from(row)
                .ok()
                .and_then(|x| x.checked_mul(lay.sweep_stride))
                .ok_or_else(|| Error::corrupt("nmr-processing", "row index overflows"))?;
            let tr = ds.read_trace(m.index, sweep, cols.0 as u64, ncol as u64)?;
            let ch = tr.channels.into_iter().next().unwrap_or_default();
            if ch.len() < ncol {
                return Err(Error::corrupt(
                    "nmr-processing",
                    format!(
                        "trace {} row {row}: {} values, expected {ncol}",
                        m.index,
                        ch.len()
                    ),
                ));
            }
            f(r, &ch[..ncol])?;
        }
    }
    Ok(())
}

/// The refusal for a read larger than `max` points, with a hint naming the CLI flags and the
/// MCP arguments that narrow it.
pub fn too_many(total: usize, max: usize) -> Error {
    Error::unsupported(
        "nmr-processing",
        format!(
            "the spectrum (or the requested part) has {total} points, more than the {max} allowed in memory"
        ),
        "Read part of it: give a shift range per dimension (`--range-ppm f1=A:B --range-ppm f2=C:D`; MCP range_ppm {\"f1\": [a, b], \"f2\": [c, d]}), or raise the limit with `--max-points N` (MCP max_points).",
    )
}

/// One slice or projection of a 2D spectrum.
#[derive(Debug, Clone)]
pub struct Slice {
    /// The matrix trace it was cut from.
    pub source_trace: u32,
    /// What was asked.
    pub spec: SliceSpec,
    /// Values along the result's axis.
    pub values: Vec<f64>,
    /// The result's axis (ppm positions).
    pub axis: PpmAxis,
    /// What the axis measures.
    pub dimension: DimensionAxis,
    /// Rows and columns: the point used along the other dimension and its position (ppm).
    pub at: Option<(usize, f64)>,
    /// A name: `pdata/1 row 233 (F1 4.120 ppm)`, `pdata/1 skyline projection onto F2`.
    pub name: String,
}

/// The 2D plane of `m`: the two fastest dimensions.
fn plane(m: &MatrixTrace) -> (usize, usize) {
    let d = m.layout.shape.len();
    (d - 2, d - 1)
}

/// Compute `spec` on the stored 2D spectrum `trace` of `ds` (default: the first one).
pub fn read_slice(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace: Option<u32>,
    spec: SliceSpec,
) -> Result<Slice> {
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let m = matrix_trace(info, &vendor, trace)?;
    slice_of(ds, &m, spec)
}

fn slice_of(ds: &mut dyn Dataset, m: &MatrixTrace, spec: SliceSpec) -> Result<Slice> {
    let (d1, d2) = plane(m);
    let dims = m.layout.shape.len();
    let (a1, a2) = (m.layout.axes[d1], m.layout.axes[d2]);
    let (n1, n2) = (a1.size, a2.size);
    let name = m
        .name
        .clone()
        .unwrap_or_else(|| format!("trace {}", m.index));
    let pick = |axis: &PpmAxis, ppm: f64, d: usize| -> Result<usize> {
        let k = axis.index_of(ppm).round();
        if !(k >= 0.0 && k <= (axis.size - 1) as f64) {
            return Err(Error::Usage(format!(
                "{ppm} ppm is outside F{} ({:.4}..{:.4} ppm)",
                d + 1,
                axis.first_ppm,
                axis.last_ppm()
            )));
        }
        Ok(k as usize)
    };
    if dims > 2 && !matches!(spec, SliceSpec::Projection { .. }) {
        return Err(Error::unsupported(
            "nmr-processing",
            format!("a row or column of a {dims}D spectrum"),
            "Rows and columns are cut from 2D spectra; read one row of a 3D spectrum with `openreadout trace --sweep N`, or project it (`--slice skyline:f2`).",
        ));
    }
    let mut at = None;
    let (values, axis_dim, label) = match spec {
        SliceSpec::Row(ppm) => {
            let r = pick(&a1, ppm, 0)?;
            let mut out = Vec::new();
            for_rows(ds, m, (r, r), (0, n2 - 1), usize::MAX, &mut |_, v| {
                out = v.to_vec();
                Ok(())
            })?;
            at = Some((r, a1.ppm(r as f64)));
            let shown = m.axes[d1].show(a1.ppm(r as f64));
            (
                out,
                d2,
                format!("{name} row {r} (F1 {shown:.4} {})", m.axes[d1].unit),
            )
        }
        SliceSpec::Column(ppm) => {
            let c = pick(&a2, ppm, 1)?;
            let mut out = Vec::with_capacity(n1);
            for_rows(ds, m, (0, n1 - 1), (c, c), usize::MAX, &mut |_, v| {
                out.push(v[0]);
                Ok(())
            })?;
            at = Some((c, a2.ppm(c as f64)));
            (
                out,
                d1,
                format!("{name} column {c} (F2 {:.4} ppm)", a2.ppm(c as f64)),
            )
        }
        SliceSpec::Projection { onto, method } => {
            let n = if onto == 0 { n1 } else { n2 };
            let init = match method {
                ProjectionMethod::Skyline => f64::NEG_INFINITY,
                ProjectionMethod::Min => f64::INFINITY,
                ProjectionMethod::Sum => 0.0,
            };
            let mut out = vec![init; n];
            for_rows(ds, m, (0, n1 - 1), (0, n2 - 1), usize::MAX, &mut |r, v| {
                if onto == 1 {
                    for (o, &x) in out.iter_mut().zip(v) {
                        fold(o, x, method);
                    }
                } else {
                    for &x in v {
                        fold(&mut out[r], x, method);
                    }
                }
                Ok(())
            })?;
            for o in &mut out {
                if !o.is_finite() {
                    *o = 0.0;
                }
            }
            let what = match method {
                ProjectionMethod::Skyline => "skyline projection",
                ProjectionMethod::Min => "negative skyline (min) projection",
                ProjectionMethod::Sum => "sum projection",
            };
            (
                out,
                if onto == 0 { d1 } else { d2 },
                format!("{name} {what} onto F{}", onto + 1),
            )
        }
    };
    let axis = m.layout.axes[axis_dim];
    Ok(Slice {
        source_trace: m.index,
        spec,
        axis: PpmAxis {
            size: values.len(),
            ..axis
        },
        values,
        dimension: m.axes[axis_dim].clone(),
        at,
        name: label,
    })
}

fn fold(o: &mut f64, x: f64, method: ProjectionMethod) {
    if !x.is_finite() {
        return;
    }
    match method {
        ProjectionMethod::Skyline => *o = o.max(x),
        ProjectionMethod::Min => *o = o.min(x),
        ProjectionMethod::Sum => *o += x,
    }
}

/// What [`read_pooled`] should read.
#[derive(Debug, Clone)]
pub struct PooledRequest {
    /// Trace (default: the first stored 2D/3D spectrum).
    pub trace: Option<u32>,
    /// Shift window of F1 and F2 of the plane (ppm, either order; `None` = all of it).
    pub ranges_ppm: [Option<(f64, f64)>; 2],
    /// At most this many cells along F1 and F2.
    pub cells: (usize, usize),
    /// Refuse to read more points than this.
    pub max_points: usize,
}

impl Default for PooledRequest {
    fn default() -> Self {
        Self {
            trace: None,
            ranges_ppm: [None, None],
            cells: (512, 512),
            max_points: DEFAULT_MAX_POINTS,
        }
    }
}

/// A 2D spectrum reduced for drawing (module docs).
#[derive(Debug, Clone)]
pub struct PooledView {
    /// Trace read.
    pub trace: u32,
    /// Its name.
    pub name: Option<String>,
    /// Points per dimension of the whole spectrum.
    pub full_shape: Vec<usize>,
    /// F1 and F2 axes of the window, at full resolution.
    pub axes: [PpmAxis; 2],
    /// What F1 and F2 measure.
    pub dimensions: [DimensionAxis; 2],
    /// Cells along F1 and F2.
    pub rows: usize,
    /// Cells along F2.
    pub cols: usize,
    /// Window points per cell along F1 and F2.
    pub factor: (usize, usize),
    /// Largest value per cell, row-major, F1 first.
    pub max: Vec<f64>,
    /// Smallest value per cell.
    pub min: Vec<f64>,
    /// Skyline (max) and min projections onto F2, one value per window point.
    pub f2_projection: (Vec<f64>, Vec<f64>),
    /// The same onto F1.
    pub f1_projection: (Vec<f64>, Vec<f64>),
    /// Noise SD of the rows read (as `matrix_noise_sd`).
    pub noise_sd: f64,
    /// Largest absolute value in the window.
    pub largest: f64,
    /// Remarks.
    pub notes: Vec<String>,
}

impl PooledView {
    /// Position (ppm) of the centre of cell `k` along plane dimension `d` (0 = F1).
    pub fn cell_ppm(&self, d: usize, k: f64) -> f64 {
        let f = if d == 0 { self.factor.0 } else { self.factor.1 } as f64;
        self.axes[d].ppm(k * f + (f - 1.0) / 2.0)
    }
}

/// Read the plane of a stored 2D/3D spectrum reduced to at most `req.cells` cells by max and
/// min pooling, with skyline projections and a noise estimate (module docs).
pub fn read_pooled(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PooledRequest,
) -> Result<PooledView> {
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let m = matrix_trace(info, &vendor, req.trace)?;
    let (d1, d2) = plane(&m);
    let (a1, a2) = (m.layout.axes[d1], m.layout.axes[d2]);
    let w1 = window(&a1, req.ranges_ppm[0], d1)?;
    let w2 = window(&a2, req.ranges_ppm[1], d2)?;
    let (n1, n2) = (w1.1 - w1.0 + 1, w2.1 - w2.0 + 1);
    let f1 = n1.div_ceil(req.cells.0.max(1)).max(1);
    let f2 = n2.div_ceil(req.cells.1.max(1)).max(1);
    let (rows, cols) = (n1.div_ceil(f1), n2.div_ceil(f2));
    let mut max = vec![f64::NEG_INFINITY; rows * cols];
    let mut min = vec![f64::INFINITY; rows * cols];
    let mut p2 = (vec![f64::NEG_INFINITY; n2], vec![f64::INFINITY; n2]);
    let mut p1 = (vec![f64::NEG_INFINITY; n1], vec![f64::INFINITY; n1]);
    // rows kept for the noise estimate: up to 48, spread evenly
    let every = (n1 / 48).max(1);
    let mut noise_rows: Vec<f64> = Vec::new();
    let mut largest = 0.0f64;
    let mut cell_max = vec![f64::NEG_INFINITY; cols];
    let mut cell_min = vec![f64::INFINITY; cols];
    for_rows(ds, &m, w1, w2, req.max_points, &mut |r, v| {
        let i = r - w1.0;
        let cr = i / f1;
        for c in 0..cols {
            let (a, b) = (c * f2, ((c + 1) * f2).min(n2));
            let (mut hi, mut lo) = (f64::NEG_INFINITY, f64::INFINITY);
            for &x in &v[a..b] {
                if x.is_finite() {
                    hi = hi.max(x);
                    lo = lo.min(x);
                }
            }
            cell_max[c] = hi;
            cell_min[c] = lo;
        }
        let row = &mut max[cr * cols..(cr + 1) * cols];
        for (o, &x) in row.iter_mut().zip(&cell_max) {
            *o = o.max(x);
        }
        let row = &mut min[cr * cols..(cr + 1) * cols];
        for (o, &x) in row.iter_mut().zip(&cell_min) {
            *o = o.min(x);
        }
        let (mut hi, mut lo) = (f64::NEG_INFINITY, f64::INFINITY);
        for (k, &x) in v.iter().enumerate() {
            if x.is_finite() {
                p2.0[k] = p2.0[k].max(x);
                p2.1[k] = p2.1[k].min(x);
                hi = hi.max(x);
                lo = lo.min(x);
                largest = largest.max(x.abs());
            }
        }
        p1.0[i] = p1.0[i].max(hi);
        p1.1[i] = p1.1[i].min(lo);
        if i % every == 0 && noise_rows.len() < 48 * n2 {
            noise_rows.extend_from_slice(v);
        }
        Ok(())
    })?;
    let clean = |v: &mut Vec<f64>| {
        for x in v.iter_mut() {
            if !x.is_finite() {
                *x = 0.0;
            }
        }
    };
    for v in [
        &mut max, &mut min, &mut p2.0, &mut p2.1, &mut p1.0, &mut p1.1,
    ] {
        clean(v);
    }
    let noise_sd = matrix_noise_sd(&noise_rows, n2);
    let mut notes = m.layout.notes.clone();
    if m.layout.shape.len() > 2 {
        notes.push(format!(
            "a {}D spectrum: its F2–F3 plane is shown as the skyline over F1",
            m.layout.shape.len()
        ));
    }
    let win_axis = |a: &PpmAxis, w: (usize, usize)| PpmAxis {
        first_ppm: a.ppm(w.0 as f64),
        size: w.1 - w.0 + 1,
        ..*a
    };
    Ok(PooledView {
        trace: m.index,
        name: m.name.clone(),
        full_shape: m.layout.shape.clone(),
        axes: [win_axis(&a1, w1), win_axis(&a2, w2)],
        dimensions: [m.axes[d1].clone(), m.axes[d2].clone()],
        rows,
        cols,
        factor: (f1, f2),
        max,
        min,
        f2_projection: p2,
        f1_projection: p1,
        noise_sd,
        largest,
        notes,
    })
}

/// One derived trace of [`NmrMatrixDataset`].
struct Derived {
    spec: SliceSpec,
    info: TraceInfo,
    data: Option<Vec<f64>>,
}

/// The matrix of a 2D/3D spectrum as a long-form table.
struct MatrixTable {
    info: TableInfo,
    trace: MatrixTrace,
    /// The last matrix row read: (row, values).
    row: Option<(usize, Vec<f64>)>,
}

/// Wraps an NMR data set: slices of a stored 2D spectrum read as extra traces (after the
/// stored ones), and with [`NmrMatrixDataset::with_matrix_table`] its matrix reads as an extra
/// table (after the stored ones): one row per point, a column per dimension (`f1_ppm`,
/// `f2_ppm`; a J-resolved F1 in Hz is `f1_hz`) and `intensity`, the slowest dimension first.
/// `info` stays header-only; slices are computed on first read. Everything else is delegated.
pub struct NmrMatrixDataset {
    inner: Box<dyn Dataset>,
    derived: Vec<(u32, Derived)>,
    table: Option<(u32, MatrixTable)>,
    source: u32,
}

impl std::fmt::Debug for NmrMatrixDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NmrMatrixDataset")
            .field("source", &self.source)
            .field(
                "derived",
                &self.derived.iter().map(|d| d.0).collect::<Vec<_>>(),
            )
            .field("table", &self.table.as_ref().map(|t| t.0))
            .finish_non_exhaustive()
    }
}

impl NmrMatrixDataset {
    /// Wrap `inner` with one derived trace per entry of `slices`, cut from the stored 2D
    /// spectrum `trace` (default: the first one). Errors (exit 6) when there is none.
    pub fn new(inner: Box<dyn Dataset>, trace: Option<u32>, slices: &[SliceSpec]) -> Result<Self> {
        let info = inner.info()?;
        let vendor = inner.vendor_metadata().unwrap_or(Value::Null);
        let m = matrix_trace(&info, &vendor, trace)?;
        let (d1, d2) = plane(&m);
        let mut next = info.traces.iter().map(|t| t.index + 1).max().unwrap_or(0);
        let mut derived = Vec::new();
        for &spec in slices {
            if m.layout.shape.len() > 2 && !matches!(spec, SliceSpec::Projection { .. }) {
                return Err(Error::unsupported(
                    "nmr-processing",
                    format!("a row or column of a {}D spectrum", m.layout.shape.len()),
                    "Rows and columns are cut from 2D spectra; read one row of a 3D spectrum with `openreadout trace --sweep N`, or project it (`--slice skyline:f2`).",
                ));
            }
            let dim = if spec.along() == 0 { d1 } else { d2 };
            let axis = m.layout.axes[dim];
            let desc = &m.axes[dim];
            let name = m
                .name
                .clone()
                .unwrap_or_else(|| format!("trace {}", m.index));
            let t = TraceInfo {
                index: next,
                name: Some(format!("{name} {}", spec.text())),
                sample_rate_hz: 0.0,
                sample_count: axis.size as u64,
                sweep_count: 1,
                channels: vec![SignalChannelInfo {
                    index: 0,
                    name: "real".into(),
                    unit: None,
                    dtype: "float64".into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: Default::default(),
                }],
                start_s: None,
                extra: [
                    ("kind".to_string(), json!("processed_spectrum")),
                    ("slice_of".to_string(), json!(m.index)),
                    ("slice".to_string(), json!(spec.text())),
                    (
                        "axis".to_string(),
                        json!({
                            "quantity": desc.quantity,
                            "unit": "ppm",
                            "first": axis.first_ppm,
                            "step": axis.step_ppm,
                            "last": axis.last_ppm(),
                            "size": axis.size,
                            "spectrometer_frequency_mhz": axis.reference_frequency_mhz,
                        }),
                    ),
                ]
                .into_iter()
                .chain(
                    desc.nucleus
                        .as_ref()
                        .map(|n| ("nucleus".to_string(), json!(n))),
                )
                .collect(),
            };
            derived.push((
                next,
                Derived {
                    spec,
                    info: t,
                    data: None,
                },
            ));
            next += 1;
        }
        Ok(Self {
            inner,
            derived,
            table: None,
            source: m.index,
        })
    }

    /// Also present the matrix as a long-form table (type docs).
    pub fn with_matrix_table(mut self) -> Result<Self> {
        let info = self.inner.info()?;
        let vendor = self.inner.vendor_metadata().unwrap_or(Value::Null);
        let m = matrix_trace(&info, &vendor, Some(self.source))?;
        let rows = m
            .layout
            .shape
            .iter()
            .try_fold(1u64, |a, &b| a.checked_mul(b as u64))
            .ok_or_else(|| Error::corrupt("nmr-processing", "matrix size overflows"))?;
        let mut columns: Vec<ColumnInfo> = m
            .axes
            .iter()
            .enumerate()
            .map(|(d, a)| ColumnInfo {
                index: d as u32,
                name: format!("f{}_{}", d + 1, a.slug()),
                label: Some(match &a.nucleus {
                    Some(n) => format!("F{} ({n}), {}", d + 1, a.unit),
                    None => format!("F{}, {}", d + 1, a.unit),
                }),
                dtype: "float64".into(),
                unit: Some(a.unit.clone()),
                range: None,
                extra: Default::default(),
            })
            .collect();
        columns.push(ColumnInfo {
            index: columns.len() as u32,
            name: "intensity".into(),
            label: Some("intensity (real part)".into()),
            dtype: "float64".into(),
            unit: None,
            range: None,
            extra: Default::default(),
        });
        let index = info.tables.iter().map(|t| t.index + 1).max().unwrap_or(0);
        let name = m
            .name
            .clone()
            .unwrap_or_else(|| format!("trace {}", m.index));
        let t = TableInfo {
            index,
            name: Some(format!("{name} matrix")),
            row_count: rows,
            columns,
            extra: [
                ("kind".to_string(), json!("nmr_matrix")),
                ("matrix_of".to_string(), json!(m.index)),
                ("shape".to_string(), json!(m.layout.shape)),
            ]
            .into_iter()
            .collect(),
        };
        self.table = Some((
            index,
            MatrixTable {
                info: t,
                trace: m,
                row: None,
            },
        ));
        Ok(self)
    }

    /// The trace index of derived trace `i` (in the order the slices were given).
    pub fn slice_trace(&self, i: usize) -> Option<u32> {
        self.derived.get(i).map(|d| d.0)
    }

    /// The table index of the matrix table, when there is one.
    pub fn matrix_table(&self) -> Option<u32> {
        self.table.as_ref().map(|t| t.0)
    }

    /// The stored 2D spectrum the slices are cut from.
    pub fn source_trace(&self) -> u32 {
        self.source
    }

    fn derived_values(&mut self, index: u32) -> Result<Option<&Vec<f64>>> {
        let Some(pos) = self.derived.iter().position(|d| d.0 == index) else {
            return Ok(None);
        };
        if self.derived[pos].1.data.is_none() {
            let info = self.inner.info()?;
            let spec = self.derived[pos].1.spec;
            let s = read_slice(self.inner.as_mut(), &info, Some(self.source), spec)?;
            self.derived[pos].1.data = Some(s.values);
        }
        Ok(self.derived[pos].1.data.as_ref())
    }
}

impl Dataset for NmrMatrixDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut info = self.inner.info()?;
        for (_, d) in &self.derived {
            info.traces.push(d.info.clone());
        }
        if let Some((_, t)) = &self.table {
            info.tables.push(t.info.clone());
        }
        if !self.derived.is_empty() {
            info.notes.push(format!(
                "traces {} are slices or projections of trace {} computed by OpenReadout (see traces[].extra.slice)",
                self.derived
                    .iter()
                    .map(|d| d.0.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                self.source
            ));
        }
        Ok(info)
    }
    fn vendor_metadata(&self) -> Result<Value> {
        self.inner.vendor_metadata()
    }
    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        self.inner.assurance_observations()
    }
    fn file_assurance(&self) -> Option<openreadout_core::assurance::Assurance> {
        self.inner.file_assurance()
    }
    fn is_strict(&self) -> bool {
        self.inner.is_strict()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.inner.read_plane(image, index)
    }
    fn check(&mut self) -> Result<CheckReport> {
        self.inner.check()
    }
    fn check_headers(&mut self) -> Result<CheckReport> {
        self.inner.check_headers()
    }
    fn member_files(&self) -> Vec<std::path::PathBuf> {
        self.inner.member_files()
    }
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let Some((ti, mt)) = self.table.as_mut().filter(|t| t.0 == index) else {
            return self.inner.read_table(index, first_row, max_rows);
        };
        let ti = *ti;
        let lay = &mt.trace.layout;
        let dims = lay.shape.len();
        let n_last = lay.shape[dims - 1];
        let total = mt.info.row_count;
        let a = first_row.min(total);
        let b = first_row.saturating_add(max_rows).min(total);
        let mut columns: Vec<Vec<f64>> = vec![Vec::with_capacity((b - a) as usize); dims + 1];
        let mut k = a;
        while k < b {
            let row = (k / n_last as u64) as usize;
            let col = (k % n_last as u64) as usize;
            if mt.row.as_ref().is_none_or(|r| r.0 != row) {
                let sweep = u32::try_from(row)
                    .ok()
                    .and_then(|x| x.checked_mul(lay.sweep_stride))
                    .ok_or_else(|| Error::corrupt("nmr-processing", "row index overflows"))?;
                let tr = self
                    .inner
                    .read_trace(mt.trace.index, sweep, 0, n_last as u64)?;
                let ch = tr.channels.into_iter().next().unwrap_or_default();
                if ch.len() < n_last {
                    return Err(Error::corrupt(
                        "nmr-processing",
                        format!("row {row}: {} values, expected {n_last}", ch.len()),
                    ));
                }
                mt.row = Some((row, ch));
            }
            let values = &mt.row.as_ref().map(|r| &r.1).cloned().unwrap_or_default();
            // indices of the slower dimensions of this row (odometer, slowest first)
            let mut idx = vec![0usize; dims];
            let mut rem = row;
            for d in (0..dims - 1).rev() {
                idx[d] = rem % lay.shape[d];
                rem /= lay.shape[d];
            }
            let end = (b - k).min((n_last - col) as u64) as usize;
            for j in col..col + end {
                idx[dims - 1] = j;
                for d in 0..dims {
                    columns[d].push(mt.trace.axes[d].show(lay.axes[d].ppm(idx[d] as f64)));
                }
                columns[dims].push(values[j]);
            }
            k += end as u64;
        }
        Ok(Table {
            table: ti,
            first_row: a,
            columns,
        })
    }
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if !self.derived.iter().any(|d| d.0 == index) {
            return self
                .inner
                .read_trace(index, sweep, first_sample, max_samples);
        }
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "trace {index} is a slice with one sweep (sweep 0), not sweep {sweep}"
            )));
        }
        let v = self.derived_values(index)?.cloned().unwrap_or_default();
        let n = v.len() as u64;
        let a = first_sample.min(n) as usize;
        let b = first_sample.saturating_add(max_samples).min(n) as usize;
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: a as u64,
            channels: vec![v[a..b].to_vec()],
        })
    }
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        self.inner.experiment()
    }
}

/// What an export of an NMR data set reads instead of what was named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportSelection {
    /// The trace to export (a slice's derived trace), if it changed.
    pub trace: Option<u32>,
    /// The table to export (the matrix table), if it changed.
    pub table: Option<u32>,
}

/// The export of an NMR data set (`export`, MCP `openreadout_export`): with `slice`, that slice
/// of the 2D spectrum `trace` as a trace; else a `csv`, `parquet` or `arrow` export (`format`
/// `None` = the default) of a stored or processed 2D/3D spectrum, named by `trace` or the
/// first one when no trace, table or sweep is named, writes the matrix in long form (one row
/// per point, a column per dimension, `intensity`); JCAMP-DX writes 2D spectra itself.
/// `None` when nothing changes.
pub fn export_view(
    ds: Box<dyn Dataset>,
    trace: Option<u32>,
    table: Option<u32>,
    sweep: Option<u32>,
    slice: Option<&str>,
    format: Option<&str>,
) -> Result<(Box<dyn Dataset>, Option<ExportSelection>)> {
    if let Some(s) = slice {
        let spec = SliceSpec::parse(s)?;
        let w = NmrMatrixDataset::new(ds, trace, &[spec])?;
        let t = w.slice_trace(0);
        return Ok((
            Box::new(w),
            Some(ExportSelection {
                trace: t,
                table: None,
            }),
        ));
    }
    let info = ds.info()?;
    let tabular = matches!(format, None | Some("csv" | "parquet" | "arrow"));
    if table.is_some() || sweep.is_some() || !tabular {
        return Ok((ds, None));
    }
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let matrix = info
        .traces
        .iter()
        .filter(|t| trace.is_none_or(|i| t.index == i))
        .find(|t| matrix_layout(t, &vendor).is_some())
        .map(|t| t.index);
    let Some(t) = matrix else {
        return Ok((ds, None));
    };
    let w = NmrMatrixDataset::new(ds, Some(t), &[])?.with_matrix_table()?;
    let table = w.matrix_table();
    Ok((Box::new(w), Some(ExportSelection { trace: None, table })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::model::FormatDescriptor;
    use openreadout_core::provenance::Confidence;

    /// A file description with these traces.
    pub(crate) fn file(traces: Vec<TraceInfo>) -> FileInfo {
        FileInfo {
            path: "fake".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "bruker-nmr".into(),
                name: "Fake NMR".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "nmr".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces,
            plane_count: 0,
            notes: vec![],
        }
    }

    /// A 2D spectrum in memory: 6 F1 rows (13C, 120 → 70 ppm) × 8 F2 points (1H, 8 → 1 ppm).
    struct Fake {
        data: Vec<f64>,
    }

    const N1: usize = 6;
    const N2: usize = 8;

    fn fake() -> Fake {
        let mut data = vec![0.0; N1 * N2];
        data[2 * N2 + 3] = 10.0; // F1 row 2, F2 point 3
        data[4 * N2 + 6] = -4.0;
        Fake { data }
    }

    impl Dataset for Fake {
        fn info(&self) -> Result<FileInfo> {
            let ax = |first: f64, step: f64, n: usize, mhz: f64| json!({"unit": "ppm", "quantity": "chemical_shift", "first": first, "step": step, "size": n, "spectrometer_frequency_mhz": mhz});
            let t = TraceInfo {
                index: 0,
                name: Some("pdata/1".into()),
                sample_count: N2 as u64,
                sweep_count: N1 as u32,
                channels: vec![SignalChannelInfo::default()],
                extra: [
                    ("kind".to_string(), json!("processed_spectrum")),
                    ("axis".to_string(), ax(8.0, -1.0, N2, 400.0)),
                    ("nucleus".to_string(), json!("1H")),
                    ("shape".to_string(), json!([N1, N2])),
                    (
                        "indirect_axes".to_string(),
                        json!([ax(120.0, -10.0, N1, 100.0)]),
                    ),
                    ("sweep_axis".to_string(), json!({"nucleus": "13C"})),
                ]
                .into_iter()
                .collect(),
                ..TraceInfo::default()
            };
            Ok(file(vec![t]))
        }
        fn vendor_metadata(&self) -> Result<Value> {
            Ok(Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::default()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(Vec::new())
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Err(Error::Usage("no planes".into()))
        }
        fn check(&mut self) -> Result<CheckReport> {
            Err(Error::Usage("no check".into()))
        }
        fn read_trace(&mut self, _: u32, sweep: u32, first: u64, n: u64) -> Result<Trace> {
            let r = sweep as usize;
            let a = first as usize;
            let b = (a + n as usize).min(N2);
            Ok(Trace {
                trace: 0,
                sweep,
                first_sample: first,
                channels: vec![self.data[r * N2 + a..r * N2 + b].to_vec()],
            })
        }
    }

    #[test]
    fn slices_parse_and_print() {
        assert_eq!(SliceSpec::parse("row:4.1").unwrap(), SliceSpec::Row(4.1));
        assert_eq!(
            SliceSpec::parse(" Skyline : F2 ").unwrap(),
            SliceSpec::Projection {
                onto: 1,
                method: ProjectionMethod::Skyline
            }
        );
        for bad in ["row", "row:x", "skyline:f3", "diagonal:1", "column:inf"] {
            assert!(SliceSpec::parse(bad).is_err(), "{bad}");
        }
        let s = SliceSpec::parse("sum:f1").unwrap();
        assert_eq!(SliceSpec::parse(&s.text()).unwrap(), s);
    }

    #[test]
    fn rows_columns_and_projections() {
        let mut ds = fake();
        let info = ds.info().unwrap();
        let row = read_slice(&mut ds, &info, None, SliceSpec::Row(100.0)).unwrap();
        assert_eq!(row.values.len(), N2);
        assert_eq!(row.values[3], 10.0);
        assert_eq!(row.at.unwrap().0, 2);
        assert_eq!(row.dimension.nucleus.as_deref(), Some("1H"));
        let col = read_slice(&mut ds, &info, None, SliceSpec::Column(5.0)).unwrap();
        assert_eq!(col.values, vec![0.0, 0.0, 10.0, 0.0, 0.0, 0.0]);
        assert!((col.axis.first_ppm - 120.0).abs() < 1e-12);
        let sky = read_slice(
            &mut ds,
            &info,
            None,
            SliceSpec::Projection {
                onto: 1,
                method: ProjectionMethod::Skyline,
            },
        )
        .unwrap();
        assert_eq!(sky.values[3], 10.0);
        assert_eq!(sky.values[6], 0.0);
        let low = read_slice(
            &mut ds,
            &info,
            None,
            SliceSpec::Projection {
                onto: 0,
                method: ProjectionMethod::Min,
            },
        )
        .unwrap();
        assert_eq!(low.values[4], -4.0);
        let e = read_slice(&mut ds, &info, None, SliceSpec::Row(500.0)).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
    }

    #[test]
    fn pooling_keeps_a_one_point_peak() {
        let mut ds = fake();
        let info = ds.info().unwrap();
        let v = read_pooled(
            &mut ds,
            &info,
            &PooledRequest {
                cells: (2, 2),
                ..PooledRequest::default()
            },
        )
        .unwrap();
        assert_eq!((v.rows, v.cols), (2, 2));
        assert_eq!(v.max.iter().copied().fold(f64::MIN, f64::max), 10.0);
        assert_eq!(v.min.iter().copied().fold(f64::MAX, f64::min), -4.0);
        assert_eq!(v.largest, 10.0);
        assert_eq!(v.f2_projection.0[3], 10.0);
        // a crop
        let v = read_pooled(
            &mut ds,
            &info,
            &PooledRequest {
                ranges_ppm: [Some((110.0, 80.0)), Some((6.0, 2.0))],
                ..PooledRequest::default()
            },
        )
        .unwrap();
        assert_eq!((v.rows, v.cols), (4, 5));
        assert!((v.axes[0].first_ppm - 110.0).abs() < 1e-12);
        // the guard
        let e = read_pooled(
            &mut ds,
            &info,
            &PooledRequest {
                max_points: 10,
                ..PooledRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert!(e.hint().unwrap_or_default().contains("--range-ppm"));
    }

    #[test]
    fn the_wrapper_adds_slices_and_the_matrix_table() {
        let ds = NmrMatrixDataset::new(
            Box::new(fake()),
            None,
            &[
                SliceSpec::Column(5.0),
                SliceSpec::parse("skyline:f2").unwrap(),
            ],
        )
        .unwrap()
        .with_matrix_table()
        .unwrap();
        let mut ds = ds;
        let info = ds.info().unwrap();
        assert_eq!(info.traces.len(), 3);
        assert_eq!(ds.slice_trace(0), Some(1));
        let t = ds.read_trace(1, 0, 0, 100).unwrap();
        assert_eq!(t.channels[0][2], 10.0);
        assert!(ds.read_trace(1, 1, 0, 100).is_err());
        let ti = ds.matrix_table().unwrap();
        let table = &info.tables[ti as usize];
        assert_eq!(table.row_count, (N1 * N2) as u64);
        assert_eq!(
            table
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["f1_ppm", "f2_ppm", "intensity"]
        );
        // rows 18..21 straddle F1 row 2: (2, 2), (2, 3) = 10, (2, 4)
        let rows = ds.read_table(ti, 18, 3).unwrap();
        assert_eq!(rows.columns[0], vec![100.0, 100.0, 100.0]);
        assert_eq!(rows.columns[1], vec![6.0, 5.0, 4.0]);
        assert_eq!(rows.columns[2], vec![0.0, 10.0, 0.0]);
        let all = ds.read_table(ti, 0, 1000).unwrap();
        assert_eq!(all.columns[2].len(), N1 * N2);
    }

    #[test]
    fn jresolved_and_dosy_axes_are_named_from_the_file() {
        let ax = |first: f64, step: f64, n: usize, mhz: f64| json!({"unit": "ppm", "first": first, "step": step, "size": n, "spectrometer_frequency_mhz": mhz});
        let t = TraceInfo {
            sample_count: 16,
            sweep_count: 4,
            extra: [
                ("kind".to_string(), json!("processed_spectrum")),
                ("procno".to_string(), json!(1)),
                ("axis".to_string(), ax(10.0, -0.5, 16, 700.0)),
                ("nucleus".to_string(), json!("1H")),
                ("shape".to_string(), json!([4, 16])),
                (
                    "indirect_axes".to_string(),
                    json!([ax(0.03, -0.02, 4, 700.0)]),
                ),
            ]
            .into_iter()
            .collect(),
            ..TraceInfo::default()
        };
        let vendor = json!({"pdata": {"1": {"proc2s": {"parameters": {"AXNUC": "1H"}}}}});
        let lay = matrix_layout(&t, &vendor).unwrap();
        let info = file(vec![]);
        let a = dimension_axes(&info, &t, &vendor, &lay);
        assert_eq!(a[0].quantity, "coupling");
        assert_eq!(a[0].unit, "Hz");
        assert!((a[0].show(0.01) - 7.0).abs() < 1e-9);
        assert_eq!(a[1].unit, "ppm");
        let vendor = json!({"pdata": {"1": {"proc2s": {"parameters": {"AXNUC": "1H", "AXUNIT": "log(m2/s)"}}}}});
        let a = dimension_axes(&info, &t, &vendor, &lay);
        assert_eq!(a[0].quantity, "other");
        assert_eq!(a[0].unit, "log(m2/s)");
        assert_eq!(a[0].slug(), "log_m2_s");
    }
}
