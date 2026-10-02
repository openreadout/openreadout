//! `analyze peaks`: chromatograms → peak tables, targeted picks, manual integrations and compound
//! results, as flat rows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::model::FileInfo;
use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Result};

use crate::bands::{RegionBaseline, SpectrumBands};
use crate::extract::{ChromRequest, Chromatogram, Target, Tolerance, extract};
use crate::peaks::{Peak, PeakMethod, PeakParams, PickRule, find_peaks, integrate_range, pick};
use crate::targets::{Compound, CompoundResult};

/// Default half-width of a compound's retention-time window, minutes.
pub const DEFAULT_RT_WINDOW_MIN: f64 = 0.5;

/// What `analyze peaks` does.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PeaksRequest {
    /// The chromatograms to analyse and the scan filters (as for `analyze chromatogram`).
    pub chrom: ChromRequest,
    /// Detection and integration parameters.
    pub params: PeakParams,
    /// Report the peak nearest this retention time: `(rt, window)` minutes.
    pub expect: Option<(f64, f64)>,
    /// How the expected peak is chosen.
    pub pick: PickRule,
    /// Manual integrations `[start, end]`, minutes.
    pub integrate: Vec<[f64; 2]>,
    /// Targeted compounds (one result row each).
    pub compounds: Vec<Compound>,
    /// Window half-width for compounds without one, minutes.
    pub default_window_min: Option<f64>,
    /// Axis windows `[a, b]` (either order) to integrate: regions of spectra (axis units:
    /// cm⁻¹, nm, ppm, …), manual integrations of chromatograms (minutes).
    pub x_ranges: Vec<[f64; 2]>,
    /// Baseline under those windows and manual integrations: `linear` (default) or `none`.
    pub region_baseline: RegionBaseline,
}

/// The expected peak of a chromatogram (`--rt`/`--window`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PickedPeak {
    /// Expected retention time, minutes.
    pub expected_rt_min: f64,
    /// Window half-width, minutes.
    pub rt_window_min: f64,
    /// `largest` or `nearest`.
    pub rule: String,
    /// The peak, when one has its apex in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak: Option<Peak>,
}

/// Peaks of one chromatogram.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChromatogramPeaks {
    /// Chromatogram label (see `analyze chromatogram`).
    pub label: String,
    /// `tic`, `bpc`, `xic`, `srm` or `trace`.
    pub kind: String,
    /// `spectra`, `trace N` or `table N`.
    pub source: String,
    /// XIC m/z.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mz: Option<f64>,
    /// XIC window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mz_window: Option<[f64; 2]>,
    /// SRM precursor matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// SRM product matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_mz: Option<f64>,
    /// Signal unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity_unit: Option<String>,
    /// Unit of peak areas, e.g. `mAU·min`.
    pub area_unit: String,
    /// Samples in the chromatogram.
    pub points: u64,
    /// How the peaks were found.
    pub method: PeakMethod,
    /// Peaks found.
    pub peak_count: u32,
    /// Summed area.
    pub total_area: f64,
    /// Number of the largest peak.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_peak: Option<u32>,
    /// Its area % (purity by area normalization).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_peak_area_percent: Option<f64>,
    /// The peak table.
    pub peaks: Vec<Peak>,
    /// The expected peak (`--rt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picked: Option<PickedPeak>,
    /// Manual integrations (`--integrate`), in request order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<Peak>,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Output of `analyze peaks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PeaksOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// One entry per analysed chromatogram.
    pub chromatograms: Vec<ChromatogramPeaks>,
    /// One row per compound of the compound list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compounds: Vec<CompoundResult>,
    /// Spectra (traces whose axis is not time: wavenumber, wavelength, ppm, …): bands and
    /// integrated regions, one entry per analysed spectrum.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectra: Vec<SpectrumBands>,
    /// Files written (`-o`, `--plot`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One peak as a flat row, with the file and chromatogram it belongs to
/// (`analyze peaks -o rows.csv`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PeakRow {
    /// The input file.
    pub path: String,
    /// Chromatogram label.
    pub chromatogram: String,
    /// Unit of `area`.
    pub area_unit: String,
    /// The peak.
    #[serde(flatten)]
    pub peak: Peak,
}

/// One band of a spectrum as a flat row (`analyze peaks -o rows.csv` on spectra).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BandRow {
    /// The input file.
    pub path: String,
    /// Spectrum label.
    pub spectrum: String,
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Axis unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_unit: Option<String>,
    /// Unit of `area`.
    pub area_unit: String,
    /// The band.
    #[serde(flatten)]
    pub band: crate::bands::Band,
}

/// One integrated region of a spectrum as a flat row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RegionRow {
    /// The input file.
    pub path: String,
    /// Spectrum label.
    pub spectrum: String,
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Axis unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_unit: Option<String>,
    /// Unit of `area`.
    pub area_unit: String,
    /// The region.
    #[serde(flatten)]
    pub region: crate::bands::Region,
}

impl PeaksOutput {
    /// Every band of every spectrum as a flat row.
    pub fn band_rows(&self) -> Vec<BandRow> {
        self.spectra
            .iter()
            .flat_map(|s| {
                s.peaks.iter().map(|b| BandRow {
                    path: self.path.clone(),
                    spectrum: s.label.clone(),
                    trace: s.trace,
                    sweep: s.sweep,
                    x_unit: s.x_unit.clone(),
                    area_unit: s.area_unit.clone(),
                    band: b.clone(),
                })
            })
            .collect()
    }

    /// Every integrated region of every spectrum as a flat row.
    pub fn region_rows(&self) -> Vec<RegionRow> {
        self.spectra
            .iter()
            .flat_map(|s| {
                s.regions.iter().map(|g| RegionRow {
                    path: self.path.clone(),
                    spectrum: s.label.clone(),
                    trace: s.trace,
                    sweep: s.sweep,
                    x_unit: s.x_unit.clone(),
                    area_unit: s.area_unit.clone(),
                    region: g.clone(),
                })
            })
            .collect()
    }

    /// Every peak of every chromatogram as a flat row.
    pub fn peak_rows(&self) -> Vec<PeakRow> {
        self.chromatograms
            .iter()
            .flat_map(|c| {
                c.peaks.iter().map(|p| PeakRow {
                    path: self.path.clone(),
                    chromatogram: c.label.clone(),
                    area_unit: c.area_unit.clone(),
                    peak: p.clone(),
                })
            })
            .collect()
    }
}

fn area_unit(c: &Chromatogram, p: &PeakParams) -> String {
    format!(
        "{}·{}",
        c.intensity_unit.as_deref().unwrap_or("counts"),
        p.area_time_unit.id()
    )
}

fn analyse(c: &Chromatogram, req: &PeaksRequest) -> Result<ChromatogramPeaks> {
    let table = find_peaks(&c.rt_min, &c.intensity, &req.params)?;
    let mut out = ChromatogramPeaks {
        label: c.label.clone(),
        kind: c.kind.clone(),
        source: c.source.clone(),
        mz: c.mz,
        mz_window: c.mz_window,
        precursor_mz: c.precursor_mz,
        product_mz: c.product_mz,
        intensity_unit: c.intensity_unit.clone(),
        area_unit: area_unit(c, &req.params),
        points: c.rt_min.len() as u64,
        method: table.method.clone(),
        peak_count: table.peak_count,
        total_area: table.total_area,
        main_peak: table.main_peak,
        main_peak_area_percent: table.main_peak_area_percent,
        peaks: table.peaks.clone(),
        picked: None,
        manual: Vec::new(),
        notes: c.notes.clone(),
    };
    if let Some((rt, w)) = req.expect {
        out.picked = Some(PickedPeak {
            expected_rt_min: rt,
            rt_window_min: w,
            rule: match req.pick {
                PickRule::Largest => "largest",
                PickRule::Nearest => "nearest",
            }
            .into(),
            peak: pick(&table, rt, w, req.pick).cloned(),
        });
    }
    let zero = (req.region_baseline == RegionBaseline::None).then_some((0.0, 0.0));
    for &[a, b] in &req.integrate {
        out.manual.push(integrate_range(
            &c.rt_min,
            &c.intensity,
            a,
            b,
            zero,
            &req.params,
        )?);
    }
    for &[a, b] in &req.x_ranges {
        out.manual.push(integrate_range(
            &c.rt_min,
            &c.intensity,
            a.min(b),
            a.max(b),
            zero,
            &req.params,
        )?);
    }
    Ok(out)
}

/// Run `analyze peaks` on one open file. Returns the output and the chromatograms analysed (for
/// plots and exports).
pub fn run_peaks(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PeaksRequest,
    ctx: &ReadContext<'_>,
) -> Result<(PeaksOutput, Vec<Chromatogram>)> {
    if let Some((rt, w)) = req.expect
        && !(rt.is_finite() && w.is_finite() && w > 0.0)
    {
        return Err(Error::Usage(
            "--rt needs a retention time and --window a half-width > 0 (minutes)".into(),
        ));
    }
    let mut out = PeaksOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        ..PeaksOutput::default()
    };
    // Traces whose axis is not time are spectra: bands and regions instead of peaks.
    let (spectra, rest) = split_spectra(info, &req.chrom);
    if !spectra.is_empty() {
        if !req.compounds.is_empty() {
            return Err(Error::Usage(
                "a compound list (--targets) applies to chromatograms; spectra are integrated over axis windows (--x-range A:B)".into(),
            ));
        }
        if req.expect.is_some() {
            return Err(Error::Usage(
                "--rt applies to chromatograms; for a band of a spectrum give its window with --x-range A:B".into(),
            ));
        }
        let mut regions = req.x_ranges.clone();
        regions.extend(req.integrate.iter().copied());
        let mut chroms = Vec::new();
        for (trace, channel) in spectra {
            let s = crate::bands::read_spectrum(ds, info, trace, channel, req.chrom.sweep)?;
            let b = crate::bands::analyse(&s, &req.params, &regions, req.region_baseline)?;
            chroms.push(crate::bands::as_chromatogram(&s));
            out.spectra.push(b);
        }
        match rest {
            None => return Ok((out, chroms)),
            Some(r) => {
                let mut sub = req.clone();
                sub.chrom = r;
                let (more, c) = run_peaks(ds, info, &sub, ctx)?;
                out.chromatograms = more.chromatograms;
                out.notes.extend(more.notes);
                chroms.extend(c);
                return Ok((out, chroms));
            }
        }
    }
    if req.compounds.is_empty() {
        let ext = extract(ds, info, &req.chrom, ctx)?;
        out.notes.extend(ext.notes);
        for c in &ext.chromatograms {
            out.chromatograms.push(analyse(c, req)?);
        }
        return Ok((out, ext.chromatograms));
    }
    // Compounds: one extraction per distinct scan filter, all XICs of a group in one pass.
    let default = req.chrom.targets.first().cloned().unwrap_or(
        if info.spectra.is_empty() && !info.traces.is_empty() {
            Target::Trace {
                trace: 0,
                channel: None,
            }
        } else {
            Target::Tic
        },
    );
    let mut groups: Vec<(ChromRequest, Vec<usize>)> = Vec::new();
    for (i, c) in req.compounds.iter().enumerate() {
        let mut r = req.chrom.clone();
        r.targets = vec![compound_target(c, &default)];
        if let Some(p) = c.polarity {
            r.polarity = Some(p);
        }
        if let Some(l) = c.ms_level {
            r.ms_level = Some(l);
        }
        if let Some(p) = c.precursor_mz {
            r.precursor_mz = Some(p);
        }
        if let Some(p) = c.ppm {
            r.tolerance = Tolerance::Ppm(p);
        } else if let Some(d) = c.da {
            r.tolerance = Tolerance::Da(d);
        }
        let key = |x: &ChromRequest| {
            (
                x.polarity,
                x.ms_level,
                x.precursor_mz.map(f64::to_bits),
                format!("{:?}", x.tolerance),
            )
        };
        match groups.iter_mut().find(|(g, _)| key(g) == key(&r)) {
            Some((g, idx)) => {
                if !g.targets.contains(&r.targets[0]) {
                    g.targets.push(r.targets[0].clone());
                }
                idx.push(i);
            }
            None => groups.push((r, vec![i])),
        }
    }
    let mut chroms: Vec<Chromatogram> = Vec::new();
    let mut analysed: Vec<ChromatogramPeaks> = Vec::new();
    let mut by_compound: Vec<Option<usize>> = vec![None; req.compounds.len()];
    for (g, idx) in &groups {
        let ext = extract(ds, info, g, ctx)?;
        for &i in idx {
            let want = compound_target(&req.compounds[i], &default);
            let k = g.targets.iter().position(|t| *t == want).unwrap_or(0);
            let Some(c) = ext.chromatograms.get(k) else {
                continue;
            };
            let pos = chroms.iter().position(|x| {
                x.label == c.label && x.polarity == c.polarity && x.source == c.source
            });
            let pos = if let Some(p) = pos {
                p
            } else {
                let mut plain = req.clone();
                plain.expect = None;
                plain.integrate.clear();
                analysed.push(analyse(c, &plain)?);
                chroms.push(c.clone());
                chroms.len() - 1
            };
            by_compound[i] = Some(pos);
        }
    }
    for (i, c) in req.compounds.iter().enumerate() {
        let Some(k) = by_compound[i] else { continue };
        let a = &analysed[k];
        let window = c
            .rt_window_min
            .or(req.default_window_min)
            .unwrap_or(DEFAULT_RT_WINDOW_MIN);
        let mut row = CompoundResult {
            path: info.path.clone(),
            compound: c.name.clone(),
            chromatogram: a.label.clone(),
            expected_rt_min: c.rt_min,
            rt_window_min: c.rt_min.map(|_| window),
            area_unit: Some(a.area_unit.clone()),
            ..CompoundResult::default()
        };
        let table = crate::peaks::PeakTable {
            method: a.method.clone(),
            peak_count: a.peak_count,
            total_area: a.total_area,
            main_peak: a.main_peak,
            main_peak_area_percent: a.main_peak_area_percent,
            peaks: a.peaks.clone(),
        };
        let found = match c.rt_min {
            Some(rt) => pick(&table, rt, window, req.pick),
            None => a.main_peak.and_then(|n| a.peaks.get(n as usize - 1)),
        };
        match found {
            Some(p) => {
                row.found = true;
                row.rt_min = Some(p.rt_min);
                row.rt_shift_min = c.rt_min.map(|rt| p.rt_min - rt);
                row.area = Some(p.area);
                row.height = Some(p.height);
                row.snr = p.snr;
                row.width_half_min = p.width_half_min;
                row.tailing_factor = p.tailing_factor;
                row.start_min = Some(p.start_min);
                row.end_min = Some(p.end_min);
                row.baseline_code = Some(p.baseline_code.clone());
                row.area_percent = Some(p.area_percent);
            }
            None => {
                row.note = Some(if a.points == 0 {
                    "no data points (no scan matched the compound's filters)".into()
                } else if a.peak_count == 0 {
                    format!(
                        "no peak above S/N {} in the chromatogram",
                        req.params.min_snr
                    )
                } else {
                    format!(
                        "no peak with its apex within {} ± {} min ({} peaks elsewhere)",
                        c.rt_min.unwrap_or(0.0),
                        window,
                        a.peak_count
                    )
                });
            }
        }
        out.compounds.push(row);
    }
    out.chromatograms = analysed;
    Ok((out, chroms))
}

/// The spectra among the requested targets (trace, channel), and the request for the rest
/// (`None` when nothing else was asked for). With no targets the default is trace 0 when the
/// file has traces and no mass spectra, as for chromatograms.
fn split_spectra(
    info: &FileInfo,
    r: &ChromRequest,
) -> (Vec<(u32, Option<u32>)>, Option<ChromRequest>) {
    let is_spectrum = |trace: u32| {
        info.traces
            .iter()
            .find(|t| t.index == trace)
            .and_then(crate::bands::spectral_axis)
            .is_some()
    };
    if r.targets.is_empty() {
        if info.spectra.is_empty() && !info.traces.is_empty() && is_spectrum(0) {
            return (vec![(0, None)], None);
        }
        return (Vec::new(), Some(r.clone()));
    }
    let mut spectra = Vec::new();
    let mut rest = r.clone();
    rest.targets.clear();
    for t in &r.targets {
        match t {
            Target::Trace { trace, channel } if is_spectrum(*trace) => {
                spectra.push((*trace, *channel));
            }
            other => rest.targets.push(other.clone()),
        }
    }
    let rest = (!rest.targets.is_empty()).then_some(rest);
    (spectra, rest)
}

fn compound_target(c: &Compound, default: &Target) -> Target {
    if let (Some(q1), Some(q3)) = (c.q1, c.q3) {
        Target::Srm { q1, q3 }
    } else if let Some(mz) = c.mz {
        Target::Xic { mz }
    } else if let Some(trace) = c.trace {
        Target::Trace {
            trace,
            channel: c.channel,
        }
    } else {
        default.clone()
    }
}
