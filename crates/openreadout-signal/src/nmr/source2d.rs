//! Reading a stored 2D or 3D spectrum out of any NMR reader as a row-major matrix, the slowest
//! dimension first, with one [`PpmAxis`] and nucleus per dimension.
//!
//! What each reader exposes (`info` → `traces[]`):
//!
//! | reader | trace | dimensions and axes | rows |
//! | --- | --- | --- | --- |
//! | Bruker (`bruker-nmr`) | `pdata/<n>` with `extra.shape` | `extra.shape` (slowest first); direct `extra.axis`, indirect `extra.indirect_axes` (fastest first after the direct one); nuclei `AXNUC` of `procs`, `proc2s`, `proc3s` | sweep = one row of the direct dimension, rows in row-major order |
//! | JEOL (`jeol-jdf`) | `kind` `processed_spectrum` with `extra.indirect_dimensions[]` | direct `extra.axis`, indirect `indirect_dimensions[0].axis` (ppm) | sweep per F1 row; with twice as many sweeps as F1 points, the even ones (real part) |
//! | JCAMP-DX (`jcamp-dx`) | `nD NMR SPECTRUM` NTUPLES with `extra.sweep_axis` | direct `extra.axis` (Hz or ppm); F1 = the page values (Hz), converted with the F1 frequency: the observe frequency × Ξ(F1)/Ξ(F2) (IUPAC 2001) from `##.NUCLEUS=` | one page per F1 row |
//!
//! Only the rows and columns inside `ranges_ppm` are read (`read_trace` per row with a column
//! window), so a crop of a large spectrum costs only the crop. The matrix is one `Vec<f64>`;
//! [`MatrixRequest::max_points`] refuses (exit 6) a crop larger than that, with a hint to narrow
//! the ranges.
//!
//! The carrier of each indirect dimension is taken at point n/2 of its full axis (quadrature
//! detection puts zero frequency at the centre of a processed axis, as in the direct dimension).

use serde_json::Value;

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::{Dataset, Error, Result};

use crate::nmr::process::PpmAxis;
use crate::nmr::source::{is_fid, spectrum_axis};

/// Default [`MatrixRequest::max_points`]: 2²⁸ points (2 GiB of `f64`).
pub const DEFAULT_MAX_POINTS: usize = 1 << 28;

/// IUPAC (2001) frequency ratios Ξ, % of the 1H frequency in TMS, for the nuclei of
/// heteronuclear 2D experiments. Used only when a file gives one dimension's frequency.
pub const FREQUENCY_RATIOS: &[(&str, f64)] = &[
    ("1H", 100.0),
    ("2H", 15.350_609),
    ("13C", 25.145_020),
    ("15N", 10.136_767),
    ("19F", 94.094_011),
    ("29Si", 19.867_187),
    ("31P", 40.480_742),
];

/// Ξ of `nucleus` (`13C`, `^13C`, `C13` are all understood), if listed.
pub fn frequency_ratio(nucleus: &str) -> Option<f64> {
    let n = normalize_nucleus(nucleus)?;
    FREQUENCY_RATIOS
        .iter()
        .find(|(k, _)| *k == n)
        .map(|&(_, v)| v)
}

/// `^13C`, `<13C>`, `C13`, ` 13c ` → `13C`; `None` for empty or `off`.
pub fn normalize_nucleus(s: &str) -> Option<String> {
    let t = s.trim().trim_matches(['<', '>', '^']).trim();
    if t.is_empty() || t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("none") {
        return None;
    }
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    let letters: String = t.chars().filter(char::is_ascii_alphabetic).collect();
    if digits.is_empty() || letters.is_empty() {
        return Some(t.to_string());
    }
    let mut el = letters.to_ascii_lowercase();
    if let Some(f) = el.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    Some(format!("{digits}{el}"))
}

/// How a trace maps onto a matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixLayout {
    /// Points per dimension, slowest first.
    pub shape: Vec<usize>,
    /// Axis per dimension, slowest first.
    pub axes: Vec<PpmAxis>,
    /// Nucleus per dimension, slowest first.
    pub nuclei: Vec<Option<String>>,
    /// Sweeps per matrix row (2 when the reader interleaves real and imaginary F1 rows).
    pub sweep_stride: u32,
    /// Remarks (inferred conversions).
    pub notes: Vec<String>,
}

fn axis_from(v: &Value, fallback_size: usize) -> Option<PpmAxis> {
    let unit = v.get("unit")?.as_str()?;
    if !unit.eq_ignore_ascii_case("ppm") {
        return None;
    }
    let size = v
        .get("size")
        .and_then(Value::as_u64)
        .map_or(fallback_size, |s| s as usize);
    Some(PpmAxis {
        first_ppm: v.get("first")?.as_f64()?,
        step_ppm: v.get("step")?.as_f64()?,
        size,
        reference_frequency_mhz: v
            .get("spectrometer_frequency_mhz")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    })
}

fn text<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// The matrix layout of a frequency-domain trace with at least two dimensions, if it has one.
/// `vendor` is the reader's `vendor_metadata` (nuclei of Bruker indirect dimensions, JCAMP-DX
/// `##.NUCLEUS=`); `Value::Null` works with fewer nuclei known.
#[allow(clippy::needless_range_loop, clippy::too_many_lines)]
pub fn matrix_layout(t: &TraceInfo, vendor: &Value) -> Option<MatrixLayout> {
    if is_fid(t) {
        return None;
    }
    let direct = spectrum_axis(t)?;
    let direct = PpmAxis {
        size: (t.sample_count as usize).min(direct.size),
        ..direct
    };
    let direct_nucleus = t
        .extra
        .get("nucleus")
        .and_then(Value::as_str)
        .and_then(normalize_nucleus);
    let sweeps = t.sweep_count as usize;
    // Bruker: shape + indirect_axes
    if let Some(shape) = t.extra.get("shape").and_then(Value::as_array) {
        let shape: Option<Vec<usize>> = shape
            .iter()
            .map(|v| v.as_u64().map(|x| x as usize))
            .collect();
        let shape = shape?;
        let ind = t.extra.get("indirect_axes").and_then(Value::as_array)?;
        if shape.len() < 2 || ind.len() + 1 != shape.len() {
            return None;
        }
        let dims = shape.len();
        // indirect_axes is fastest first: entry k is dimension dims-2-k
        let mut axes = Vec::with_capacity(dims);
        for d in 0..dims - 1 {
            let k = dims - 2 - d;
            axes.push(axis_from(ind.get(k)?, shape[d])?);
        }
        axes.push(direct);
        let procno = t
            .extra
            .get("procno")
            .and_then(Value::as_u64)
            .map(|p| p.to_string());
        let mut nuclei = Vec::with_capacity(dims);
        for d in 0..dims - 1 {
            let k = dims - 2 - d; // proc2s for k = 0
            let file = format!("proc{}s", k + 2);
            let from_tree = procno.as_ref().and_then(|p| {
                text(vendor, &format!("/pdata/{p}/{file}/parameters/AXNUC"))
                    .and_then(normalize_nucleus)
            });
            let from_sweep = (dims == 2)
                .then(|| {
                    t.extra
                        .get("sweep_axis")
                        .and_then(|s| s.get("nucleus"))
                        .and_then(Value::as_str)
                        .and_then(normalize_nucleus)
                })
                .flatten();
            nuclei.push(from_tree.or(from_sweep));
        }
        nuclei.push(direct_nucleus);
        for (d, a) in axes.iter_mut().enumerate() {
            a.size = shape[d];
        }
        let rows: usize = shape[..dims - 1].iter().product();
        if rows != sweeps || shape[dims - 1] != direct.size {
            return None;
        }
        return Some(MatrixLayout {
            shape,
            axes,
            nuclei,
            sweep_stride: 1,
            notes: Vec::new(),
        });
    }
    // JEOL: indirect_dimensions[0].axis in ppm
    if let Some(ind) = t
        .extra
        .get("indirect_dimensions")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        let ax = ind.get("axis").and_then(|a| axis_from(a, 0))?;
        let n1 = if ax.size > 0 { ax.size } else { sweeps };
        let stride = if n1 * 2 == sweeps {
            2
        } else if n1 == sweeps {
            1
        } else {
            return None;
        };
        let nucleus = ind
            .get("nucleus")
            .and_then(Value::as_str)
            .and_then(normalize_nucleus);
        return Some(MatrixLayout {
            shape: vec![n1, direct.size],
            axes: vec![PpmAxis { size: n1, ..ax }, direct],
            nuclei: vec![nucleus, direct_nucleus],
            sweep_stride: stride,
            notes: Vec::new(),
        });
    }
    // JCAMP-DX: pages keyed by F1 in Hz
    let sa = t.extra.get("sweep_axis")?;
    let values: Vec<f64> = sa
        .get("values")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    if values.len() < 2 || values.len() != sweeps {
        return None;
    }
    let n1 = values.len();
    let mut notes = Vec::new();
    // the declared F1 range (first `##FIRST=`/`##LAST=` entries, `##VAR_DIM=`) when it has as
    // many points as there are pages; else the page values, which must be regular
    let num = |k: &str| {
        jcamp_list(vendor, k)
            .first()
            .and_then(|x| x.parse::<f64>().ok())
    };
    let declared = match (num("FIRST"), num("LAST"), num("VAR_DIM")) {
        (Some(a), Some(b), Some(n)) if n as usize == n1 && (a - b).abs() > 0.0 => {
            Some((a, (b - a) / (n1 - 1) as f64))
        }
        _ => None,
    };
    let from_pages = (values[0], (values[n1 - 1] - values[0]) / (n1 - 1) as f64);
    let regular = from_pages.1 != 0.0
        && values.iter().enumerate().all(|(k, v)| {
            (v - (from_pages.0 + k as f64 * from_pages.1)).abs() <= 0.05 * from_pages.1.abs()
        });
    let (first, step) = match declared {
        Some(d) => {
            if !regular
                || (d.0 - from_pages.0).abs() > d.1.abs()
                || (d.1 - from_pages.1).abs() > 0.01 * d.1.abs()
            {
                notes.push(format!(
                    "the F1 axis is the declared range (##FIRST= {:.4}, step {:.4}); the page values (first {:.4}, step {:.4}) disagree with it",
                    d.0, d.1, from_pages.0, from_pages.1
                ));
            }
            d
        }
        None if regular => from_pages,
        None => return None,
    };
    let values = [first, first + step * (n1 - 1) as f64];
    let unit = sa.get("unit").and_then(Value::as_str).unwrap_or_default();
    let jcamp_nuclei = jcamp_nuclei(vendor);
    let f1_nucleus = jcamp_nuclei.first().cloned().flatten();
    let f1_axis = if unit.eq_ignore_ascii_case("ppm") {
        PpmAxis {
            first_ppm: values[0],
            step_ppm: step,
            size: n1,
            reference_frequency_mhz: 0.0,
        }
    } else if unit.eq_ignore_ascii_case("hz") {
        let f2 = direct_nucleus
            .clone()
            .or_else(|| jcamp_nuclei.last().cloned().flatten());
        let (Some(a), Some(b)) = (
            f1_nucleus.as_deref().and_then(frequency_ratio),
            f2.as_deref().and_then(frequency_ratio),
        ) else {
            return None;
        };
        let mhz = direct.reference_frequency_mhz * a / b;
        if !(mhz > 0.0) {
            return None;
        }
        notes.push(format!(
            "the F1 axis is given in Hz; its frequency {mhz:.4} MHz is inferred from the observe frequency and the nuclei's frequency ratios (IUPAC 2001), not read from the file"
        ));
        PpmAxis {
            first_ppm: values[0] / mhz,
            step_ppm: step / mhz,
            size: n1,
            reference_frequency_mhz: mhz,
        }
    } else {
        return None;
    };
    let direct_nucleus = direct_nucleus.or_else(|| jcamp_nuclei.last().cloned().flatten());
    Some(MatrixLayout {
        shape: vec![n1, direct.size],
        axes: vec![f1_axis, direct],
        nuclei: vec![f1_nucleus, direct_nucleus],
        sweep_stride: 1,
        notes,
    })
}

/// The first `##<key>=` record of the JCAMP-DX vendor tree (key compared without `#`, `=`,
/// spaces and underscores, case-insensitively), split at commas.
fn jcamp_list(vendor: &Value, key: &str) -> Vec<String> {
    let norm = |l: &str| -> String {
        l.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect::<String>()
            .to_ascii_uppercase()
    };
    let want = norm(key);
    let Some(blocks) = vendor.get("blocks").and_then(Value::as_array) else {
        return Vec::new();
    };
    for b in blocks {
        let Some(recs) = b.get("records").and_then(Value::as_array) else {
            continue;
        };
        for r in recs {
            let (Some(label), Some(value)) = (
                r.get(0).and_then(Value::as_str),
                r.get(1).and_then(Value::as_str),
            ) else {
                continue;
            };
            if norm(label) == want {
                return value.split(',').map(|x| x.trim().to_string()).collect();
            }
        }
    }
    Vec::new()
}

/// `##.NUCLEUS=` of the JCAMP-DX vendor tree, in symbol order (F1 first).
fn jcamp_nuclei(vendor: &Value) -> Vec<Option<String>> {
    jcamp_list(vendor, ".NUCLEUS")
        .iter()
        .map(|s| normalize_nucleus(s))
        .collect()
}

/// Index of the trace to use: `requested` if given (checked), else the first stored spectrum
/// with two or more dimensions.
pub fn choose_matrix_trace(info: &FileInfo, vendor: &Value, requested: Option<u32>) -> Result<u32> {
    if let Some(i) = requested {
        let t = info.traces.iter().find(|t| t.index == i).ok_or_else(|| {
            Error::Usage(format!(
                "trace {i} out of range (file has {} traces)",
                info.traces.len()
            ))
        })?;
        if matrix_layout(t, vendor).is_none() {
            return Err(Error::unsupported(
                "nmr-processing",
                format!(
                    "trace {i} is not a stored 2D or 3D spectrum with a ppm axis per dimension"
                ),
                "Run `openreadout info <file>` and pick a processed spectrum whose extra has shape and indirect_axes (Bruker pdata/<n> with 2rr or 3rrr), a JEOL processed 2D spectrum or a JCAMP-DX nD NMR SPECTRUM.",
            ));
        }
        return Ok(i);
    }
    info.traces
        .iter()
        .find(|t| matrix_layout(t, vendor).is_some())
        .map(|t| t.index)
        .ok_or_else(|| {
            let fid2d = info
                .traces
                .iter()
                .any(|t| is_fid(t) && t.sweep_count > 1);
            Error::unsupported(
                "nmr-processing",
                format!("this {} file has no stored 2D or 3D spectrum", info.format.name),
                if fid2d {
                    "The data set has a 2D FID (ser) but no processed spectrum: process it in the vendor software first, or pick 1D peaks on one row with `analyze nmr-peaks --sweep N`."
                } else {
                    "2D peak picking needs a processed 2D/3D spectrum (Bruker pdata/<n>/2rr or 3rrr, JEOL processed 2D, JCAMP-DX nD NMR SPECTRUM); `openreadout info <file>` lists the traces."
                },
            )
        })
}

/// What [`load_matrix`] should read.
#[derive(Debug, Clone)]
pub struct MatrixRequest {
    /// Trace (default: the first stored 2D/3D spectrum).
    pub trace: Option<u32>,
    /// Read only these shift ranges, one per dimension, slowest first (ppm, either order;
    /// `None` or a missing entry = the whole dimension).
    pub ranges_ppm: Vec<Option<(f64, f64)>>,
    /// Extra points read beyond each range on both sides (footprints near the range edge).
    pub margin_points: usize,
    /// Refuse a crop with more points than this.
    pub max_points: usize,
}

impl Default for MatrixRequest {
    fn default() -> Self {
        Self {
            trace: None,
            ranges_ppm: Vec::new(),
            margin_points: 8,
            max_points: DEFAULT_MAX_POINTS,
        }
    }
}

/// A stored spectrum as a matrix.
#[derive(Debug, Clone)]
pub struct StoredMatrix {
    /// Values, row-major, slowest dimension first.
    pub data: Vec<f64>,
    /// Points per dimension of `data`.
    pub shape: Vec<usize>,
    /// Axis per dimension of `data` (of the crop).
    pub axes: Vec<PpmAxis>,
    /// Nucleus per dimension.
    pub nuclei: Vec<Option<String>>,
    /// Points per dimension of the whole spectrum.
    pub full_shape: Vec<usize>,
    /// First point of the crop per dimension in the whole spectrum.
    pub offsets: Vec<usize>,
    /// Carrier point per dimension of the crop (indirect dimensions only).
    pub carrier_points: Vec<Option<f64>>,
    /// Trace index.
    pub trace: u32,
    /// Remarks.
    pub notes: Vec<String>,
}

/// Read the matrix `req` asks for (module docs).
pub fn load_matrix(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &MatrixRequest,
) -> Result<StoredMatrix> {
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let index = choose_matrix_trace(info, &vendor, req.trace)?;
    let t = info
        .traces
        .iter()
        .find(|t| t.index == index)
        .ok_or_else(|| Error::Usage(format!("trace {index} out of range")))?;
    let lay = matrix_layout(t, &vendor).ok_or_else(|| {
        Error::unsupported(
            "nmr-processing",
            format!("trace {index} has no matrix layout"),
            "Pick a stored 2D/3D spectrum (see `openreadout info`).",
        )
    })?;
    let dims = lay.shape.len();
    // index window per dimension
    let mut win = Vec::with_capacity(dims);
    for d in 0..dims {
        let n = lay.shape[d];
        let (lo, hi) = match req.ranges_ppm.get(d).copied().flatten() {
            Some((a, b)) => {
                let (i, j) = (lay.axes[d].index_of(a), lay.axes[d].index_of(b));
                let (i, j) = if i <= j { (i, j) } else { (j, i) };
                let m = req.margin_points as f64;
                let lo = (i.floor() - m).clamp(0.0, (n - 1) as f64) as usize;
                let hi = (j.ceil() + m).clamp(0.0, (n - 1) as f64) as usize;
                if j < 0.0 || i > (n - 1) as f64 {
                    return Err(Error::Usage(format!(
                        "range {a}..{b} ppm is outside dimension {d} ({:.3}..{:.3} ppm)",
                        lay.axes[d].first_ppm,
                        lay.axes[d].last_ppm()
                    )));
                }
                (lo, hi)
            }
            None => (0, n - 1),
        };
        win.push((lo, hi));
    }
    let shape: Vec<usize> = win.iter().map(|&(a, b)| b - a + 1).collect();
    let total = shape
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .unwrap_or(usize::MAX);
    if total > req.max_points {
        return Err(Error::unsupported(
            "nmr-processing",
            format!(
                "the spectrum (or the requested part) has {total} points, more than the {} allowed in memory",
                req.max_points
            ),
            "Narrow the shift ranges per dimension (ranges_ppm) to read part of the spectrum.",
        ));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(total).map_err(|_| {
        Error::unsupported(
            "nmr-processing",
            format!("not enough memory for {total} points"),
            "Narrow the shift ranges per dimension (ranges_ppm) to read part of the spectrum.",
        )
    })?;
    let (col0, ncol) = (win[dims - 1].0, shape[dims - 1]);
    // rows: odometer over the indirect dimensions of the window
    let full_rows: Vec<usize> = lay.shape[..dims - 1].to_vec();
    let mut c: Vec<usize> = win[..dims - 1].iter().map(|w| w.0).collect();
    loop {
        let mut row = 0usize;
        for (d, &x) in c.iter().enumerate() {
            row = row * full_rows[d] + x;
        }
        let sweep = u32::try_from(row)
            .ok()
            .and_then(|r| r.checked_mul(lay.sweep_stride))
            .ok_or_else(|| Error::corrupt("nmr-processing", "row index overflows"))?;
        let tr = ds.read_trace(index, sweep, col0 as u64, ncol as u64)?;
        let ch = tr.channels.into_iter().next().unwrap_or_default();
        if ch.len() < ncol {
            return Err(Error::corrupt(
                "nmr-processing",
                format!(
                    "trace {index} row {row}: {} values, expected {ncol}",
                    ch.len()
                ),
            ));
        }
        data.extend_from_slice(&ch[..ncol]);
        // advance
        let mut d = c.len();
        let done = loop {
            if d == 0 {
                break true;
            }
            d -= 1;
            if c[d] < win[d].1 {
                c[d] += 1;
                break false;
            }
            c[d] = win[d].0;
        };
        if done {
            break;
        }
    }
    let axes: Vec<PpmAxis> = lay
        .axes
        .iter()
        .zip(&win)
        .zip(&shape)
        .map(|((a, w), &n)| PpmAxis {
            first_ppm: a.ppm(w.0 as f64),
            size: n,
            ..*a
        })
        .collect();
    let carrier_points = (0..dims)
        .map(|d| (d + 1 < dims).then(|| (lay.shape[d] / 2) as f64 - win[d].0 as f64))
        .collect();
    Ok(StoredMatrix {
        data,
        shape,
        axes,
        nuclei: lay.nuclei,
        full_shape: lay.shape,
        offsets: win.iter().map(|w| w.0).collect(),
        carrier_points,
        trace: index,
        notes: lay.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuclei_are_normalized() {
        assert_eq!(normalize_nucleus("^13C").as_deref(), Some("13C"));
        assert_eq!(normalize_nucleus("<15N>").as_deref(), Some("15N"));
        assert_eq!(normalize_nucleus("C13").as_deref(), Some("13C"));
        assert_eq!(normalize_nucleus(" 1h ").as_deref(), Some("1H"));
        assert_eq!(normalize_nucleus("off"), None);
        assert!((frequency_ratio("13C").unwrap() - 25.14502).abs() < 1e-9);
        assert!(frequency_ratio("99Xx").is_none());
    }

    fn trace(extra: serde_json::Value, samples: u64, sweeps: u32) -> TraceInfo {
        TraceInfo {
            index: 0,
            sample_count: samples,
            sweep_count: sweeps,
            extra: extra.as_object().unwrap().clone().into_iter().collect(),
            ..TraceInfo::default()
        }
    }

    #[test]
    fn bruker_layout_orders_dimensions_slowest_first() {
        let ax = |first: f64, n: u64| serde_json::json!({"unit": "ppm", "first": first, "step": -0.01, "size": n, "spectrometer_frequency_mhz": 400.0});
        let t = trace(
            serde_json::json!({
                "kind": "processed_spectrum", "procno": 1, "axis": ax(10.0, 16), "nucleus": "1H",
                "shape": [4, 8, 16], "indirect_axes": [ax(20.0, 8), ax(30.0, 4)],
            }),
            16,
            32,
        );
        let vendor = serde_json::json!({"pdata": {"1": {
            "proc2s": {"parameters": {"AXNUC": "13C"}},
            "proc3s": {"parameters": {"AXNUC": "15N"}},
        }}});
        let l = matrix_layout(&t, &vendor).unwrap();
        assert_eq!(l.shape, vec![4, 8, 16]);
        assert_eq!(
            l.axes.iter().map(|a| a.first_ppm).collect::<Vec<_>>(),
            vec![30.0, 20.0, 10.0]
        );
        assert_eq!(
            l.nuclei,
            vec![Some("15N".into()), Some("13C".into()), Some("1H".into())]
        );
        // sweeps that do not fill the shape: no layout
        let bad = TraceInfo {
            sweep_count: 31,
            ..t.clone()
        };
        assert!(matrix_layout(&bad, &vendor).is_none());
    }

    #[test]
    fn jcamp_pages_in_hz_become_a_ppm_axis() {
        let values: Vec<f64> = (0..4).map(|k| 17000.0 - 4000.0 * f64::from(k)).collect();
        let t = trace(
            serde_json::json!({
                "kind": "ntuples", "nucleus": "^1H", "observe_frequency_mhz": 400.0,
                "axis": {"unit": "HZ", "first": 4000.0, "step": -10.0, "size": 8},
                "sweep_axis": {"variable": "F1", "unit": "HZ", "values": values},
            }),
            8,
            4,
        );
        let vendor = serde_json::json!({"blocks": [{"records": [["##.NUCLEUS=", "13C,\t1H"]]}]});
        let l = matrix_layout(&t, &vendor).unwrap();
        assert_eq!(l.shape, vec![4, 8]);
        assert_eq!(l.nuclei, vec![Some("13C".into()), Some("1H".into())]);
        let mhz = 400.0 * 25.145_02 / 100.0;
        assert!((l.axes[0].first_ppm - 17000.0 / mhz).abs() < 1e-9);
        assert!((l.axes[1].first_ppm - 10.0).abs() < 1e-12);
        assert_eq!(l.notes.len(), 1);
        // a declared range that the page values contradict wins, with a note
        let vendor = serde_json::json!({"blocks": [{"records": [
            ["##.NUCLEUS=", "13C,\t1H"], ["##VAR_DIM=", "4, 8, 32"],
            ["##FIRST=", "17000, 4000, 0"], ["##LAST=", "5, -70, 0"],
        ]}]});
        let l = matrix_layout(&t, &vendor).unwrap();
        assert!((l.axes[0].first_ppm - 17000.0 / mhz).abs() < 1e-9);
        assert!((l.axes[0].last_ppm() - 5.0 / mhz).abs() < 1e-9);
        assert_eq!(l.notes.len(), 2);
    }
}
