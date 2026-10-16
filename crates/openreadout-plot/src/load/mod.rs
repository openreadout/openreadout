//! Reading the data a series names: from a file through the readers (and the analyses), from a
//! table file, or from inline values.

mod assay;
mod columns;
mod flow;
mod frames;
mod image;
mod plate;
mod signal;
mod table;

use std::collections::HashMap;
use std::path::Path;

use openreadout_core::{Dataset, Error, FileInfo, Registry, Result};

use crate::model::Series;
use crate::spec::{SeriesSpec, Show};

pub(crate) use plate::{PlateHints, seconds_in, time_unit};
pub(crate) use signal::fmt_at;

/// Shared state while loading the series of one figure.
pub(crate) struct Ctx<'a> {
    pub reg: &'a Registry,
    /// Notes for the result (what was done to the data).
    pub notes: Vec<String>,
    /// Warnings for the result (data that could not be drawn, suspicious choices).
    pub warnings: Vec<String>,
    /// TIC apex (retention time, minutes) per file, for `rt: "apex"`.
    pub apex: HashMap<String, f64>,
    /// Plate layout facts for a plate facet.
    pub plate: Option<PlateHints>,
    /// The time unit the panel's x axis is asked to show (`s`, `min`, `h`).
    pub x_unit: Option<String>,
    /// Data points read so far for the figure, against [`crate::MAX_POINTS`].
    pub points: usize,
}

impl<'a> Ctx<'a> {
    pub fn new(reg: &'a Registry) -> Ctx<'a> {
        Ctx {
            reg,
            notes: Vec::new(),
            warnings: Vec::new(),
            apex: HashMap::new(),
            plate: None,
            x_unit: None,
            points: 0,
        }
    }

    /// Count `n` more data points against the figure's budget; refuse when it runs out, before
    /// the data is read where the loader knows the size up front.
    pub fn spend(&mut self, n: usize) -> Result<()> {
        self.points = self.points.saturating_add(n);
        if self.points > crate::MAX_POINTS {
            return Err(usage(format!(
                "the figure would hold more than {} data points; draw fewer files, sweeps, wells or m/z values, or set x_range to a window",
                crate::MAX_POINTS
            )));
        }
        Ok(())
    }

    /// Open `file` and read its header.
    pub fn open(&self, file: &str) -> Result<(String, Box<dyn Dataset>, FileInfo)> {
        let (det, ds) = self.reg.open(Path::new(file))?;
        let info = ds.info()?;
        Ok((det.format_id.to_string(), ds, info))
    }
}

/// A usage error.
pub(crate) fn usage(msg: impl Into<String>) -> Error {
    Error::Usage(msg.into())
}

/// Read every series `spec` names.
pub(crate) fn load(ctx: &mut Ctx<'_>, spec: &SeriesSpec) -> Result<Vec<Series>> {
    let sources = usize::from(spec.file.is_some())
        + usize::from(spec.table.is_some())
        + usize::from(spec.values.is_some());
    if sources != 1 {
        return Err(usage(
            "each data item needs exactly one of `file` (an instrument file), `table` (a .csv/.parquet table file) or `values` (inline numbers)",
        ));
    }
    let count = |s: &[Series]| s.iter().map(|x| x.geom.len()).sum::<usize>();
    if let Some(v) = &spec.values {
        let s = table::inline(spec, v)?;
        ctx.spend(count(&s))?;
        return Ok(s);
    }
    if let Some(t) = &spec.table {
        let s = table::table(ctx, spec, t)?;
        ctx.spend(count(&s))?;
        return Ok(s);
    }
    let files = spec
        .file
        .as_ref()
        .map(crate::spec::OneOrMany::to_vec)
        .unwrap_or_default();
    if files.is_empty() {
        return Err(usage("`file` is an empty list"));
    }
    let mut out = Vec::new();
    for f in &files {
        let before = ctx.points;
        let mut s = load_file(ctx, spec, f)?;
        // loaders that cannot tell the size before reading are counted once they have read
        ctx.spend(count(&s).saturating_sub(ctx.points - before))?;
        if files.len() > 1 {
            let stem = file_label(f);
            let one = s.len() == 1;
            for x in &mut s {
                if spec.label.is_none() {
                    x.name = if x.name.is_empty() || one || s_len_one(&x.name) {
                        stem.clone()
                    } else {
                        format!("{stem} · {}", x.name)
                    };
                }
                if x.facet.is_none() {
                    x.facet = Some(stem.clone());
                }
            }
        }
        out.extend(s);
    }
    Ok(out)
}

/// Names that say nothing once there are several files (one TIC per file is named by its file).
fn s_len_one(name: &str) -> bool {
    matches!(name, "TIC" | "BPC" | "events" | "f–I")
}

/// A short name for a file: its stem.
pub(crate) fn file_label(f: &str) -> String {
    let p = Path::new(f.trim_end_matches(['/', '\\']));
    let name = p
        .file_name()
        .map_or_else(|| f.to_string(), |n| n.to_string_lossy().into_owned());
    // data-set directories (`run.d`, `1/`) keep their name; files lose the extension
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 && !p.is_dir() => stem.to_string(),
        _ => name,
    }
}

fn load_file(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let show = match spec.show {
        Some(s) => s,
        None => default_show(ctx, spec, file)?,
    };
    let mut series = match show {
        Show::Trace | Show::Sweeps | Show::Amplification | Show::Melt => {
            signal::trace(ctx, spec, file, show)?
        }
        Show::Chromatogram => signal::chromatogram(ctx, spec, file)?,
        Show::Spectrum => signal::spectrum(ctx, spec, file)?,
        Show::Fi => signal::fi(ctx, spec, file)?,
        Show::Wells => plate::wells(ctx, spec, file)?,
        Show::Events => flow::events(ctx, spec, file)?,
        Show::Histogram => {
            if ctx.open(file)?.0 == "fcs" {
                flow::histogram(ctx, spec, file)?
            } else {
                columns::histogram(ctx, spec, file)?
            }
        }
        Show::Frames => frames::frames(ctx, spec, file)?,
        Show::DoseResponse | Show::StandardCurve => assay::curves(ctx, spec, file, show)?,
        Show::Profile => image::profile(ctx, spec, file)?,
    };
    if let Some(l) = &spec.label
        && series.len() == 1
    {
        series[0].name.clone_from(l);
    }
    if let Some(m) = spec.mark {
        for s in &mut series {
            s.mark = m;
        }
    }
    Ok(series)
}

/// What a file shows when the series does not say.
fn default_show(ctx: &Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Show> {
    if spec.assay.is_some() {
        return Ok(Show::DoseResponse);
    }
    if spec.line.is_some() {
        return Ok(Show::Profile);
    }
    // a long plate CSV (`well`, `value`, maybe `time_s` and layout columns) is a plate
    let p = Path::new(file);
    let text_ext = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "csv" | "tsv" | "txt"));
    if text_ext
        && let Ok(head) = std::fs::File::open(p).and_then(|f| {
            let mut b = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(f, 4096), &mut b).map(|_| b)
        })
        && openreadout_assay::PlateData::is_long_csv(&String::from_utf8_lossy(&head))
    {
        return Ok(Show::Wells);
    }
    let (format, _ds, info) = ctx.open(file)?;
    if format == "fcs" {
        return Ok(if spec.y.is_some() {
            Show::Events
        } else {
            Show::Histogram
        });
    }
    if !info.spectra.is_empty() {
        return Ok(if spec.rt.is_some() || spec.scan.is_some() {
            Show::Spectrum
        } else {
            Show::Chromatogram
        });
    }
    if info.traces.iter().any(|t| {
        t.extra
            .get("kind")
            .and_then(|k| k.as_str())
            .is_some_and(|k| k.starts_with("amplification"))
    }) {
        return Ok(Show::Amplification);
    }
    let plate = info.tables.iter().any(|t| {
        t.columns.iter().any(|c| c.name == "well") && t.columns.iter().any(|c| c.name == "value")
    });
    if plate && info.traces.is_empty() {
        return Ok(Show::Wells);
    }
    // read files (FASTQ, BAM): the read-length histogram
    if info.format.family == "sequencing" && info.traces.is_empty() {
        return Ok(Show::Histogram);
    }
    if !info.traces.is_empty() {
        return Ok(if spec.spikes {
            Show::Sweeps
        } else {
            Show::Trace
        });
    }
    if plate {
        return Ok(Show::Wells);
    }
    if info.images.first().is_some_and(|im| im.size_t > 1) || format == "mdoc" {
        return Ok(Show::Frames);
    }
    Err(usage(format!(
        "{file}: nothing to plot by default ({format} file without traces, spectra, plate tables or frames); set `show`, or use openreadout_preview for images"
    )))
}

/// Glob match with `*` (case-insensitive).
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let p = pattern.to_lowercase();
    let t = text.to_lowercase();
    if !p.contains('*') {
        return p == t;
    }
    let parts: Vec<&str> = p.split('*').collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !t.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(k) = t[pos..].find(part) {
            pos += k + part.len();
        } else {
            return false;
        }
    }
    parts.last().is_none_or(|l| l.is_empty() || t.ends_with(l))
}

/// The `n` highest local maxima of `pts` (sorted by x), at least `min_sep` apart in x.
pub(crate) fn top_peaks(pts: &[(f64, f64)], n: usize, min_sep: f64) -> Vec<(f64, f64)> {
    if n == 0 || pts.len() < 3 {
        return pts
            .iter()
            .copied()
            .filter(|p| p.1.is_finite())
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .into_iter()
            .take(n)
            .collect();
    }
    let (lo, hi) = pts
        .iter()
        .map(|p| p.1)
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| {
            (a.0.min(v), a.1.max(v))
        });
    let floor = 0.03 * (hi - lo);
    let w = (pts.len() / 100).max(3);
    let mut cand: Vec<(f64, f64)> = Vec::new();
    // local maxima away from the ends that stand out from their surroundings
    for i in 1..pts.len() - 1 {
        let y = pts[i].1;
        if !y.is_finite() {
            continue;
        }
        let (left, right) = (pts[i - 1].1, pts[i + 1].1);
        if !(y >= left && y > right || (y > left && y >= right)) {
            continue;
        }
        let lmin = pts[i.saturating_sub(w)..i]
            .iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min);
        let rmin = pts[i + 1..(i + 1 + w).min(pts.len())]
            .iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min);
        if y - lmin.max(rmin) >= floor {
            cand.push(pts[i]);
        }
    }
    cand.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for c in cand {
        if out.iter().all(|o| (o.0 - c.0).abs() >= min_sep) {
            out.push(c);
            if out.len() == n {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_peaks() {
        assert!(glob("99*+", "99.9 G +"));
        assert!(glob("*G +", "99.9 G +"));
        assert!(!glob("99*-", "99.9 G +"));
        assert!(glob("lambda", "Lambda"));
        let pts: Vec<(f64, f64)> = (0..100)
            .map(|i| {
                let x = f64::from(i);
                (
                    x,
                    (-(x - 20.0).powi(2) / 4.0).exp() + 0.5 * (-(x - 70.0).powi(2) / 4.0).exp(),
                )
            })
            .collect();
        let p = top_peaks(&pts, 2, 5.0);
        assert_eq!(p.len(), 2);
        assert!((p[0].0 - 20.0).abs() < 1e-9 && (p[1].0 - 70.0).abs() < 1e-9);
        assert_eq!(file_label("/a/b/run.mzML"), "run");
    }
}
