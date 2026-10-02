//! `analyze peaks`: detect and integrate chromatographic peaks (`openreadout_quant::analyze`): peak
//! tables with area %, a targeted peak (`--rt`), manual integrations, and one row per compound
//! per file from a compound list (`--targets`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Registry, Result};
use openreadout_quant::analyze::{PeaksOutput, PeaksRequest, run_peaks};
use openreadout_quant::peaks::{AreaTimeUnit, BaselineMode, PeakParams, PickRule};
use openreadout_quant::rows::{RowFormat, render as render_rows};

use super::batch::{self, BatchArgs, Item, Spec, Stdin};
use super::chromatogram::{SourceArgs, parse_range};
use crate::ui::{self, Align, Progress};

/// Baseline construction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum BaselineArg {
    /// As drop, with peak ends that follow a sloped background (solvent tail, drift); the
    /// reason is recorded per peak.
    #[default]
    Auto,
    /// Valley-to-valley for resolved peaks, drop lines between unresolved ones.
    Drop,
    /// Every peak from its own start to its own end.
    Valley,
    /// As drop, with small peaks on a larger neighbour's flank skimmed off by a tangent.
    Tangent,
    /// `--x-range`/`--integrate` windows: a straight line between the window's ends (their
    /// default). Detected peaks keep the auto baseline.
    Linear,
    /// `--x-range`/`--integrate` windows: no baseline (the integral of the signal itself).
    /// Detected peaks keep the auto baseline.
    None,
}

/// Which peak `--rt` reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum PickArg {
    /// The tallest peak with its apex inside the window.
    #[default]
    Largest,
    /// The peak closest to the expected retention time.
    Nearest,
}

/// Arguments of `peaks`.
#[derive(Debug, clap::Args)]
pub struct PeaksArgs {
    /// Files, directories or glob patterns (several make a batch); `-` reads standard input.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    #[command(flatten)]
    pub source: SourceArgs,
    /// Savitzky–Golay smoothing window in points (odd; below 5 = no smoothing). Default: half
    /// the typical peak width at half height.
    #[arg(long, value_name = "N")]
    pub smooth: Option<u32>,
    /// Detection threshold: peak height / noise σ (robust estimate from the signal).
    #[arg(long, value_name = "S/N", default_value_t = openreadout_quant::peaks::DEFAULT_MIN_SNR)]
    pub min_snr: f64,
    /// Detection threshold in signal units (height above the peak's baseline).
    #[arg(long, value_name = "H")]
    pub min_height: Option<f64>,
    /// Smallest width at half height, minutes.
    #[arg(long, value_name = "MIN")]
    pub min_width: Option<f64>,
    /// Fewest samples from peak start to end.
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub min_points: u32,
    /// Baseline under detected peaks (auto, drop, valley, tangent; default auto), or under
    /// `--x-range`/`--integrate` windows (linear, the default there, or none).
    #[arg(long, value_enum)]
    pub baseline: Option<BaselineArg>,
    /// `--baseline tangent`: skim a peak off its neighbour when its height is below this
    /// fraction of the neighbour's.
    #[arg(long, value_name = "R", default_value_t = openreadout_quant::peaks::DEFAULT_SKIM_RATIO)]
    pub skim_ratio: f64,
    /// Noise σ in signal units (instead of the estimate).
    #[arg(long, value_name = "SIGMA")]
    pub noise: Option<f64>,
    /// Window of the running baseline that decides where peaks end, minutes.
    #[arg(long, value_name = "MIN")]
    pub baseline_window: Option<f64>,
    /// Report areas in signal × seconds (as most chromatography data systems do) instead of
    /// signal × minutes.
    #[arg(long)]
    pub area_seconds: bool,
    /// Report the peak at this expected retention time (minutes), within `--window`.
    #[arg(long, value_name = "MIN")]
    pub rt: Option<f64>,
    /// Half-width of the retention-time window of `--rt` and of compounds without one, minutes.
    /// Default 0.5.
    #[arg(long, value_name = "MIN")]
    pub window: Option<f64>,
    /// Which peak in the window `--rt` (and `--targets`) report.
    #[arg(long, value_enum, default_value_t)]
    pub pick: PickArg,
    /// Integrate between two retention times, `A-B` minutes (manual integration, straight
    /// baseline between the ends); repeatable.
    #[arg(long, value_name = "A-B")]
    pub integrate: Vec<String>,
    /// Integrate a window of the trace's own axis, `A:B` (either order), repeatable: a band or
    /// region of a spectrum in its units (IR/Raman cm⁻¹, UV-Vis nm, NMR ppm): area with the
    /// `--baseline` (linear between the ends, or none), area without it, maximum, centroid and
    /// the bands inside; on a chromatogram, minutes (as `--integrate`).
    #[arg(long = "x-range", value_name = "A:B", allow_hyphen_values = true)]
    pub x_range: Vec<String>,
    /// Compound list (CSV/TSV with a header, or JSON): `name` plus `mz` (XIC), `q1`+`q3`
    /// (SRM), `trace` (detector signal) or nothing (the default chromatogram), and optionally
    /// `rt`, `window`, `ppm`/`da`, `polarity`, `ms_level`, `precursor`. One result row per
    /// compound per file.
    #[arg(long, value_name = "FILE")]
    pub targets: Option<PathBuf>,
    /// Write the result rows of every input to one file: `.csv`, `.tsv` or `.jsonl`. Compound
    /// rows with `--targets`, otherwise one row per peak.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Plot the chromatograms with the integrated peaks shaded and numbered: a `.png`/`.jpg`
    /// file (one input), or a directory that receives `<input>.peaks.png` per input.
    #[arg(long, value_name = "FILE|DIR")]
    pub plot: Option<PathBuf>,
    /// Plot width in pixels.
    #[arg(long, default_value_t = 1200)]
    pub width: u32,
    /// Replace existing output files.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub json: bool,
}

impl Item for PeaksOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        if !self.spectra.is_empty() && self.chromatograms.is_empty() {
            let bands: u32 = self.spectra.iter().map(|s| s.peak_count).sum();
            let regions: usize = self.spectra.iter().map(|s| s.regions.len()).sum();
            return Some(if regions > 0 {
                format!("{regions} regions, {bands} bands")
            } else {
                format!("{bands} bands")
            });
        }
        if !self.compounds.is_empty() {
            let found = self.compounds.iter().filter(|c| c.found).count();
            return Some(format!("{found}/{} compounds found", self.compounds.len()));
        }
        let n: u32 = self.chromatograms.iter().map(|c| c.peak_count).sum();
        Some(format!("{n} peaks"))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
        for c in &mut self.compounds {
            c.path = to.into();
        }
    }
}

/// Parse an axis window `A:B` (also `A-B`, `A..B`, `A,B`; either order; signed numbers).
pub fn parse_x_range(s: &str) -> Result<[f64; 2]> {
    let bad = || {
        Error::Usage(format!(
            "--x-range {s:?}: expected two axis values A:B, e.g. 980:1060 (cm-1), 6.5:8 (ppm)"
        ))
    };
    let t = s.trim();
    let split = if let Some(p) = t.find("..") {
        Some((p, 2))
    } else if let Some(p) = t.find([':', ',']) {
        Some((p, 1))
    } else {
        // the first '-' that is not a sign or an exponent's
        t.char_indices()
            .skip(1)
            .find(|&(i, c)| c == '-' && !t[..i].ends_with(['e', 'E']))
            .map(|(i, _)| (i, 1))
    };
    let (pos, len) = split.ok_or_else(bad)?;
    let a: f64 = t[..pos].trim().parse().map_err(|_| bad())?;
    let b: f64 = t[pos + len..].trim().parse().map_err(|_| bad())?;
    if !(a.is_finite() && b.is_finite()) || a.total_cmp(&b).is_eq() {
        return Err(bad());
    }
    Ok([a.min(b), a.max(b)])
}

fn request(a: &PeaksArgs) -> Result<PeaksRequest> {
    let mut r = PeaksRequest::default();
    r.chrom = a.source.request()?;
    let mut p = PeakParams::default();
    p.smooth = a.smooth;
    p.min_snr = a.min_snr;
    p.min_height = a.min_height;
    p.min_width_min = a.min_width;
    p.min_points = a.min_points;
    p.baseline = match a.baseline {
        Some(BaselineArg::Drop) => BaselineMode::Drop,
        Some(BaselineArg::Valley) => BaselineMode::Valley,
        Some(BaselineArg::Tangent) => BaselineMode::Tangent,
        _ => BaselineMode::Auto,
    };
    if a.baseline == Some(BaselineArg::None) {
        r.region_baseline = openreadout_quant::bands::RegionBaseline::None;
    }
    r.x_ranges = a
        .x_range
        .iter()
        .map(|s| parse_x_range(s))
        .collect::<Result<_>>()?;
    p.skim_ratio = a.skim_ratio;
    p.noise = a.noise;
    p.baseline_window_min = a.baseline_window;
    p.rt_range_min = r.chrom.rt_range_min;
    p.area_time_unit = if a.area_seconds {
        AreaTimeUnit::S
    } else {
        AreaTimeUnit::Min
    };
    r.params = p;
    r.pick = match a.pick {
        PickArg::Largest => PickRule::Largest,
        PickArg::Nearest => PickRule::Nearest,
    };
    if let Some(rt) = a.rt {
        r.expect = Some((
            rt,
            a.window
                .unwrap_or(openreadout_quant::analyze::DEFAULT_RT_WINDOW_MIN),
        ));
    }
    r.default_window_min = a.window;
    r.integrate = a
        .integrate
        .iter()
        .map(|s| parse_range(s, "--integrate"))
        .collect::<Result<_>>()?;
    if let Some(t) = &a.targets {
        let text = std::fs::read_to_string(t).map_err(|e| Error::io(t, e))?;
        r.compounds = openreadout_quant::targets::parse_compounds(&text)?;
    }
    Ok(r)
}

fn plot_path(plot: &Path, input: &Path, batch: bool) -> PathBuf {
    if batch || plot.is_dir() {
        let stem = input
            .file_name()
            .map_or_else(|| "input".into(), |s| s.to_string_lossy().to_string());
        plot.join(format!("{stem}.peaks.png"))
    } else {
        plot.to_path_buf()
    }
}

pub fn run(reg: &Registry, a: &PeaksArgs) -> i32 {
    let req = match request(a) {
        Ok(r) => r,
        Err(e) => return crate::output::fail(a.json, &e),
    };
    let row_format = match &a.output {
        Some(o) => match RowFormat::from_path(o) {
            Some(f) => Some(f),
            None => {
                return crate::output::fail(
                    a.json,
                    &Error::Usage(format!(
                        "-o {}: use a .csv, .tsv or .jsonl file name",
                        o.display()
                    )),
                );
            }
        },
        None => None,
    };
    if let Some(o) = &a.output
        && o.exists()
        && !a.overwrite
    {
        return crate::output::fail(
            a.json,
            &Error::Usage(format!(
                "{} exists; pass --overwrite to replace it",
                o.display()
            )),
        );
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut first_input: Option<PathBuf> = None;
    let code = batch::run(
        reg,
        &a.files,
        Spec {
            json: a.json,
            batch: &a.batch,
            stdin: Stdin::Spool,
        },
        &mut |input| {
            let path = input.path;
            let (_, mut ds) = reg.open(path)?;
            let info = ds.info()?;
            let opener = || -> Result<Box<dyn Dataset>> { reg.open(path).map(|(_, d)| d) };
            let bar: OnceLock<Progress> = OnceLock::new();
            let progress = |done: u64, total: u64| {
                if !input.batch {
                    bar.get_or_init(|| Progress::new(total, "scans"))
                        .set(done, None);
                }
            };
            let (mut out, chroms) = run_peaks(
                ds.as_mut(),
                &info,
                &req,
                &ReadContext {
                    opener: Some(&opener),
                    progress: Some(&progress),
                },
            )?;
            if let Some(b) = bar.get() {
                b.finish();
            }
            if let Some(plot) = &a.plot {
                let target = plot_path(plot, path, input.batch);
                if input.batch {
                    std::fs::create_dir_all(plot).map_err(|e| Error::io(plot, e))?;
                }
                // spectra come first in `chroms`, then the chromatograms
                let overlays: Vec<openreadout_quant::analyze::ChromatogramPeaks> = out
                    .spectra
                    .iter()
                    .map(openreadout_quant::bands::plot_overlay)
                    .collect();
                let refs: Vec<Option<&openreadout_quant::analyze::ChromatogramPeaks>> = overlays
                    .iter()
                    .chain(&out.chromatograms)
                    .map(Some)
                    .collect();
                let labels: Vec<String> = out
                    .spectra
                    .iter()
                    .map(openreadout_quant::bands::axis_label)
                    .collect();
                let canvas =
                    openreadout_quant::plot::render_labelled(&chroms, &refs, &labels, a.width);
                let enc = if target.extension().is_some_and(|e| {
                    e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")
                }) {
                    openreadout_preview::Encoding::Jpeg
                } else {
                    openreadout_preview::Encoding::Png
                };
                let bytes = openreadout_preview::encode(
                    &canvas,
                    enc,
                    openreadout_preview::DEFAULT_JPEG_QUALITY,
                )?;
                openreadout_preview::write_verified(path, &target, &bytes, a.overwrite)?;
                out.outputs.push(target.display().to_string());
            }
            if a.output.is_some() {
                let label = input.item.path.display().to_string();
                let mut local = out.clone();
                local.path.clone_from(&label);
                for c in &mut local.compounds {
                    c.path.clone_from(&label);
                }
                let values: Vec<serde_json::Value> = if !local.spectra.is_empty() {
                    let mut v: Vec<serde_json::Value> =
                        if req.x_ranges.is_empty() && req.integrate.is_empty() {
                            local
                                .band_rows()
                                .iter()
                                .filter_map(|r| serde_json::to_value(r).ok())
                                .collect()
                        } else {
                            local
                                .region_rows()
                                .iter()
                                .filter_map(|r| serde_json::to_value(r).ok())
                                .collect()
                        };
                    v.extend(
                        local
                            .peak_rows()
                            .iter()
                            .filter_map(|r| serde_json::to_value(r).ok()),
                    );
                    v
                } else if req.compounds.is_empty() {
                    local
                        .peak_rows()
                        .iter()
                        .filter_map(|r| serde_json::to_value(r).ok())
                        .collect()
                } else {
                    local
                        .compounds
                        .iter()
                        .filter_map(|r| serde_json::to_value(r).ok())
                        .collect()
                };
                rows.extend(values);
                first_input.get_or_insert_with(|| path.to_path_buf());
            }
            Ok(out)
        },
        &render,
    );
    if let (Some(o), Some(fmt)) = (&a.output, row_format) {
        let input = first_input.unwrap_or_default();
        let result = render_rows(&rows, fmt).and_then(|text| {
            openreadout_preview::write_verified(&input, o, text.as_bytes(), a.overwrite)
        });
        match result {
            Ok(_) => {
                if !ui::quiet() {
                    eprintln!("wrote {} rows to {}", rows.len(), o.display());
                }
            }
            Err(e) => {
                crate::output::print_error(Some(&o.display().to_string()), &e);
                return code.max(e.exit_code());
            }
        }
    }
    code
}

fn num(v: Option<f64>, digits: usize) -> String {
    match v {
        None => "-".into(),
        Some(x) if x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) => format!("{x:.3e}"),
        Some(x) => format!("{x:.digits$}"),
    }
}

fn render_spectrum(b: &openreadout_quant::bands::SpectrumBands) -> String {
    let unit = b.x_unit.as_deref().unwrap_or("");
    let mut s = format!(
        "\n{} ({}, {} {:.4}..{:.4} {unit}): {} bands, area in {}, noise σ {} ({}), smoothing {} pts\n",
        b.label,
        b.source,
        b.x_quantity,
        b.x_min,
        b.x_max,
        b.peak_count,
        b.area_unit,
        num(Some(b.method.noise), 3),
        b.method.noise_method,
        b.method.smooth_window_points,
    );
    let rows: Vec<Vec<String>> = b
        .peaks
        .iter()
        .map(|p| {
            vec![
                p.number.to_string(),
                num(Some(p.x), 3),
                num(Some(p.start), 3),
                num(Some(p.end), 3),
                num(Some(p.height), 4),
                num(Some(p.area), 4),
                num(p.width_half, 3),
                num(p.snr, 1),
            ]
        })
        .collect();
    if !rows.is_empty() {
        s.push_str(&ui::table(
            &["#", "x", "start", "end", "height", "area", "FWHM", "S/N"],
            &[Align::Right; 8],
            &rows,
        ));
    }
    for g in &b.regions {
        s.push_str(&format!(
            "\nregion {} {}–{} {unit} ({} pts, baseline {}): area {} (no baseline {}), max {} at {}, centroid {}",
            g.number,
            num(Some(g.from), 4),
            num(Some(g.to), 4),
            g.points,
            g.baseline,
            num(Some(g.area), 5),
            num(Some(g.area_no_baseline), 5),
            num(Some(g.max_height), 4),
            num(Some(g.max_x), 3),
            num(g.centroid, 3),
        ));
    }
    for n in &b.notes {
        s.push_str(&format!("\nnote: {n}"));
    }
    s
}

fn render(o: &PeaksOutput) -> String {
    let mut s = format!("{} ({})\n", o.path, o.format);
    for b in &o.spectra {
        s.push_str(&render_spectrum(b));
    }
    for c in &o.chromatograms {
        s.push_str(&format!(
            "\n{}: {} peaks, area in {}, noise σ {} ({}), smoothing {} pts, baseline {}\n",
            c.label,
            c.peak_count,
            c.area_unit,
            num(Some(c.method.noise), 3),
            c.method.noise_method,
            c.method.smooth_window_points,
            c.method.baseline
        ));
        let rows: Vec<Vec<String>> = c
            .peaks
            .iter()
            .map(|p| {
                vec![
                    p.number.to_string(),
                    num(Some(p.rt_min), 3),
                    num(Some(p.start_min), 3),
                    num(Some(p.end_min), 3),
                    num(Some(p.area), 4),
                    num(Some(p.area_percent), 2),
                    num(Some(p.height), 4),
                    num(p.snr, 1),
                    num(p.width_half_min, 4),
                    num(p.tailing_factor, 2),
                    // auto: say when an end was placed on a sloped background
                    if p.baseline_reason.as_deref() == Some("sloped_background") {
                        format!("{} sloped", p.baseline_code)
                    } else {
                        p.baseline_code.clone()
                    },
                ]
            })
            .collect();
        if !rows.is_empty() {
            s.push_str(&ui::table(
                &[
                    "#", "RT min", "start", "end", "area", "area %", "height", "S/N", "W½ min",
                    "tailing", "type",
                ],
                &[
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Left,
                ],
                &rows,
            ));
        }
        if let Some(pk) = &c.picked {
            s.push_str(&format!(
                "\nexpected {} ± {} min: {}",
                pk.expected_rt_min,
                pk.rt_window_min,
                pk.peak.as_ref().map_or("no peak".into(), |p| format!(
                    "peak {} at {:.3} min, area {}",
                    p.number,
                    p.rt_min,
                    num(Some(p.area), 4)
                ))
            ));
        }
        for m in &c.manual {
            s.push_str(&format!(
                "\nmanual {:.3}–{:.3} min: area {}, height {}",
                m.start_min,
                m.end_min,
                num(Some(m.area), 4),
                num(Some(m.height), 4)
            ));
        }
        for n in &c.notes {
            s.push_str(&format!("\nnote: {n}"));
        }
    }
    if !o.compounds.is_empty() {
        let rows: Vec<Vec<String>> = o
            .compounds
            .iter()
            .map(|r| {
                vec![
                    r.compound.clone(),
                    r.chromatogram.clone(),
                    num(r.expected_rt_min, 3),
                    num(r.rt_min, 3),
                    num(r.area, 4),
                    num(r.height, 4),
                    num(r.snr, 1),
                    if r.found { "yes" } else { "no" }.into(),
                ]
            })
            .collect();
        s.push_str("\n\n");
        s.push_str(&ui::table(
            &[
                "compound",
                "chromatogram",
                "expected",
                "RT min",
                "area",
                "height",
                "S/N",
                "found",
            ],
            &[
                Align::Left,
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Right,
                Align::Left,
            ],
            &rows,
        ));
    }
    for p in &o.outputs {
        s.push_str(&format!("\nwrote {p}"));
    }
    s
}
