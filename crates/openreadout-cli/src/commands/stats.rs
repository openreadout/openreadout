//! `stats`: pixel statistics per plane, channel and image (`openreadout_core::stats`).

use std::path::PathBuf;
use std::sync::OnceLock;

use openreadout_core::parallel::ReadContext;
use openreadout_core::stats::{
    DEFAULT_BINS, HistogramScale, SampleStats, StatsOutput, StatsRequest, compute_stats,
};
use openreadout_core::{Dataset, Registry, Result};

use super::batch::{self, BatchArgs, Spec, Stdin};
use crate::ui::{self, Align, Progress};

/// Histogram bin spacing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ScaleArg {
    /// Equal-width bins (integers: one bin per value when there are fewer values than bins).
    #[default]
    Linear,
    /// Geometric bins from the smallest positive value to the maximum (values ≤ 0 counted apart).
    Log,
}

/// Axis of a maximum-intensity projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MipArg {
    /// Along z: one projected plane per image, channel and time point.
    Z,
    /// Along t: one projected plane per image, channel and z.
    T,
}

/// Arguments of `stats`.
#[derive(Debug, clap::Args)]
pub struct StatsArgs {
    /// Files, directories or glob patterns (several make a batch); `-` reads standard input.
    #[arg(required_unless_present = "from_index", value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    /// Only this image index.
    #[arg(long)]
    pub image: Option<u32>,
    /// Plane selection, e.g. `c=0`, `z=2-5`, `t=0,3`. Repeatable. The per-image aggregate
    /// covers exactly the selected planes.
    #[arg(long = "select")]
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution).
    #[arg(long, default_value_t = 0)]
    pub level: u32,
    /// Only this rectangle of each plane, `X,Y,WIDTH,HEIGHT` in the pixel coordinates of
    /// `--level` (tiled readers decode only the tiles it touches).
    #[arg(long, value_name = "X,Y,W,H")]
    pub region: Option<String>,
    /// Histogram bins (0 = no histograms; at most 65536).
    #[arg(long, value_name = "N", default_value_t = DEFAULT_BINS)]
    pub bins: u32,
    /// Histogram bin spacing.
    #[arg(long, value_enum, default_value_t)]
    pub scale: ScaleArg,
    /// Maximum-intensity projection first (per pixel, the largest value over the selected z
    /// planes, or time points), then the statistics of the projections: `--mip z --select c=1`
    /// = the MIP of channel 1.
    #[arg(long, value_enum, value_name = "AXIS")]
    pub mip: Option<MipArg>,
    /// Rows: per image and `channel` (default), per `image`, or also one entry per `plane`; `well`:
    /// a multi-well plate (high-content screening) as one row per (well, channel) over the
    /// well's fields of view (never-acquired planes left out, missing files skipped and
    /// counted); `field`: one row per (well, field, channel).
    #[arg(long, value_enum, default_value_t, value_name = "GRAIN")]
    pub per: PerArg,
    /// With `--per well` or `--per field`: only this well (`C05`). Repeatable.
    #[arg(long = "well", value_name = "WELL")]
    pub wells: Vec<String>,
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub tidy: super::tidy::TidyArgs,
}

/// Row grain of `stats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum PerArg {
    /// One row per image and channel.
    #[default]
    Channel,
    /// One row per image.
    Image,
    /// One row per plane.
    Plane,
    /// One row per well and channel of a multi-well plate.
    Well,
    /// One row per well, field of view and channel of a multi-well plate.
    Field,
}

pub fn run(reg: &Registry, a: &StatsArgs) -> i32 {
    let region = match a.region.as_deref().map(openreadout_core::Region::parse) {
        None => None,
        Some(Ok(r)) => Some(r),
        Some(Err(e)) => return crate::output::fail(a.json, &e),
    };
    if matches!(a.per, PerArg::Well | PerArg::Field) {
        return run_by_well(reg, a);
    }
    if !a.wells.is_empty() {
        return crate::output::fail(
            a.json,
            &openreadout_core::Error::Usage("--well needs --per well or --per field".into()),
        );
    }
    if super::tidy::wanted(reg, &a.tidy, &a.batch, &a.files, false) {
        let m = openreadout_batch::measures::StatsMeasure {
            image: a.image,
            select: a.select.clone(),
            level: a.level,
            per: match a.per {
                PerArg::Channel => openreadout_batch::measures::StatsPer::Channel,
                PerArg::Image => openreadout_batch::measures::StatsPer::Image,
                PerArg::Plane => openreadout_batch::measures::StatsPer::Plane,
                PerArg::Well | PerArg::Field => unreachable!("handled above"),
            },
            mip: a.mip.map(|m| match m {
                MipArg::Z => openreadout_core::stats::Projection::Z,
                MipArg::T => openreadout_core::stats::Projection::T,
            }),
        };
        return super::tidy::run(reg, &m, &a.files, &a.batch, &a.tidy, a.json);
    }
    let req = {
        let mut stats_request = StatsRequest::default();
        stats_request.image = a.image;
        stats_request.select = a.select.clone();
        stats_request.level = a.level;
        stats_request.region = region;
        stats_request.bins = a.bins;
        stats_request.scale = match a.scale {
            ScaleArg::Linear => HistogramScale::Linear,
            ScaleArg::Log => HistogramScale::Log,
        };
        stats_request.per_plane = a.per == PerArg::Plane;
        stats_request.mip = a.mip.map(|m| match m {
            MipArg::Z => openreadout_core::stats::Projection::Z,
            MipArg::T => openreadout_core::stats::Projection::T,
        });
        stats_request
    };
    batch::run(
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
                    bar.get_or_init(|| Progress::new(total, "planes"))
                        .set(done, None);
                }
            };
            compute_stats(
                ds.as_mut(),
                &info,
                &req,
                &ReadContext {
                    opener: Some(&opener),
                    progress: Some(&progress),
                },
            )
        },
        &render,
    )
}

fn run_by_well(reg: &Registry, a: &StatsArgs) -> i32 {
    let mut req = openreadout_core::plate::WellStatsRequest::default();
    req.select = a.select.clone();
    req.wells = a.wells.clone();
    req.per_field = a.per == PerArg::Field;
    req.level = a.level;
    let csv = a.tidy.csv;
    batch::run(
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
            let opener = || -> Result<Box<dyn Dataset>> { reg.open(path).map(|(_, d)| d) };
            let bar: OnceLock<Progress> = OnceLock::new();
            let progress = |done: u64, total: u64| {
                if !input.batch && !csv {
                    bar.get_or_init(|| Progress::new(total, "planes"))
                        .set(done, None);
                }
            };
            super::plate::by_well(
                ds.as_mut(),
                &req,
                &ReadContext {
                    opener: Some(&opener),
                    progress: Some(&progress),
                },
            )
        },
        &|o| {
            if csv {
                openreadout_core::plate::well_stats_csv(o)
                    .trim_end()
                    .to_string()
            } else {
                super::plate::render_by_well(o)
            }
        },
    )
}

fn num(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{x:.0}"),
        Some(x) if x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) => format!("{x:.3e}"),
        Some(x) => format!("{x:.3}"),
    }
}

fn pct(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v < 0.0001 {
        "<0.01%".into()
    } else {
        format!("{:.2}%", v * 100.0)
    }
}

/// A one-line histogram (block characters, 16 columns).
fn sparkline(s: &SampleStats) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let Some(h) = &s.histogram else {
        return String::new();
    };
    if h.counts.is_empty() {
        return String::new();
    }
    let cols = h.counts.len().min(16);
    let per = h.counts.len().div_ceil(cols);
    let merged: Vec<u64> = h.counts.chunks(per).map(|c| c.iter().sum()).collect();
    let max = merged.iter().copied().max().unwrap_or(0).max(1);
    merged
        .iter()
        .map(|&c| {
            if c == 0 {
                ' '
            } else {
                BLOCKS[((c as f64 / max as f64) * 7.0).round() as usize]
            }
        })
        .collect()
}

fn row(label: String, s: &SampleStats) -> Vec<String> {
    let p = s.percentiles.as_ref();
    vec![
        label,
        num(s.min),
        num(s.max),
        num(s.mean),
        num(s.std),
        num(p.map(|p| p.p1)),
        num(p.map(|p| p.p50)),
        num(p.map(|p| p.p99)),
        pct(s.zero_fraction),
        s.saturated_fraction.map_or_else(
            || "-".into(),
            |f| {
                if f > 0.0 {
                    ui::paint(ui::WARN, pct(f))
                } else {
                    pct(f)
                }
            },
        ),
        sparkline(s),
    ]
}

const HEAD: [&str; 11] = [
    "",
    "min",
    "max",
    "mean",
    "std",
    "p1",
    "p50",
    "p99",
    "zero",
    "saturated",
    "histogram",
];
const ALIGN: [Align; 11] = [
    Align::Left,
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
];

/// Planes listed in the human output before it switches to a note.
const MAX_PLANE_ROWS: usize = 64;

pub fn render(o: &StatsOutput) -> String {
    let mut rows = Vec::new();
    for im in &o.images {
        rows.push(row(
            ui::paint(
                ui::BOLD,
                format!(
                    "image {} ({} planes, {})",
                    im.image,
                    im.planes,
                    im.pixel_type.ome_name()
                ),
            ),
            &im.stats,
        ));
        for ch in o.channels.iter().filter(|c| c.image == im.image) {
            rows.push(row(
                format!(
                    "  c={}{}",
                    ch.c,
                    ch.name
                        .as_deref()
                        .map(|n| format!(" {n}"))
                        .unwrap_or_default()
                ),
                &ch.stats,
            ));
            for comp in &ch.components {
                rows.push(row(
                    format!(
                        "    {}",
                        comp.name
                            .clone()
                            .unwrap_or_else(|| format!("sample {}", comp.sample))
                    ),
                    &comp.stats,
                ));
            }
        }
    }
    let mut s = format!(
        "{} ({}){}{}\n",
        o.path,
        o.format,
        if o.level > 0 {
            format!("  level {}", o.level)
        } else {
            String::new()
        },
        if o.select.is_empty() {
            String::new()
        } else {
            format!("  select {}", o.select.join(" "))
        }
    );
    if let Some(m) = o.mip {
        s.push_str(&format!(
            "maximum-intensity projections along {} (planes listed with {} = 0)\n",
            m.name(),
            m.name()
        ));
    }
    s.push_str(&ui::table(&HEAD, &ALIGN, &rows));
    if !o.planes.is_empty() {
        s.push('\n');
        if o.planes.len() <= MAX_PLANE_ROWS {
            let rows: Vec<Vec<String>> = o
                .planes
                .iter()
                .map(|p| {
                    row(
                        format!("image {} c={} z={} t={}", p.image, p.c, p.z, p.t),
                        &p.stats,
                    )
                })
                .collect();
            s.push('\n');
            s.push_str(&ui::table(&HEAD, &ALIGN, &rows));
        } else {
            s.push_str(&format!(
                "({} planes; per-plane statistics with --json, or narrow with --select)",
                o.planes.len()
            ));
        }
    }
    for im in &o.images {
        if let (Some(l), Some("significant_bits")) = (
            im.stats.saturation_level,
            im.stats.saturation_basis.as_deref(),
        ) {
            s.push_str(&format!(
                "\n(image {}: saturated = {} = the recorded bit depth's ceiling, {})",
                im.image,
                num(Some(l)),
                im.stats.saturation_source.as_deref().unwrap_or("recorded")
            ));
        }
    }
    if o.images.iter().any(|i| !i.stats.exact) {
        s.push_str("\n(percentiles approximate to 0.2 %: more than 131,072 distinct values)");
    }
    s.trim_end().to_string()
}
