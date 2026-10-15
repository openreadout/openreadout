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
    /// Intensity profile along a line from (X0, Y0) to (X1, Y1), in the pixels of `--level`.
    #[arg(long, value_name = "X0,Y0,X1,Y1", help_heading = "Measurements")]
    pub line: Option<String>,
    /// Profile width in pixels, averaged across the line.
    #[arg(long, value_name = "PX", help_heading = "Measurements")]
    pub line_width: Option<u32>,
    /// Statistics inside a polygon, `X,Y X,Y X,Y ...` in the pixels of `--level`.
    #[arg(long, value_name = "X,Y ...", help_heading = "Measurements")]
    pub polygon: Option<String>,
    /// Count objects in `--region`, `--polygon` or the whole plane: their number, sizes,
    /// positions and mean intensity in every channel. `--objects` counts nuclei;
    /// `--objects=cells` or `--objects=spots` count those.
    #[arg(
        long,
        value_enum,
        value_name = "KIND",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "nuclei",
        help_heading = "Measurements"
    )]
    pub objects: Option<KindArg>,
    /// With `--objects`: expected object diameter in µm (default: measured from the image).
    #[arg(long, value_name = "UM", help_heading = "Measurements")]
    pub diameter: Option<f64>,
    /// With `--objects`: expected object diameter in pixels, for images without a pixel size.
    #[arg(long, value_name = "PX", help_heading = "Measurements")]
    pub diameter_px: Option<f64>,
    /// With `--objects`: threshold in raw units (spots: height above the local background).
    #[arg(long, help_heading = "Measurements")]
    pub threshold: Option<f64>,
    /// With `--objects`: how the threshold is chosen.
    #[arg(long, value_enum, help_heading = "Measurements")]
    pub method: Option<MethodArg>,
    /// With `--objects`: bright or dark objects (default: from the image).
    #[arg(long, value_enum, help_heading = "Measurements")]
    pub polarity: Option<PolarityArg>,
    /// With `--objects` on an RGB image: what objects are found in (default: hematoxylin for
    /// stained brightfield, the brightest component for fluorescence).
    #[arg(long, value_enum, help_heading = "Measurements")]
    pub stain: Option<StainArg>,
    /// With `--objects`: smallest object kept, in µm² (default: a fifth of the expected area).
    #[arg(long, value_name = "UM2", help_heading = "Measurements")]
    pub min_area: Option<f64>,
    /// With `--objects`: largest object kept, in µm².
    #[arg(long, value_name = "UM2", help_heading = "Measurements")]
    pub max_area: Option<f64>,
    /// With `--objects`: objects listed, in reading order (default 100).
    #[arg(long, value_name = "N", help_heading = "Measurements")]
    pub max_objects: Option<usize>,
    /// With `--objects`: keep touching objects together instead of cutting them apart.
    #[arg(long, help_heading = "Measurements")]
    pub no_split: bool,
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub tidy: super::tidy::TidyArgs,
}

/// What `--objects` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum KindArg {
    /// Nuclei (default); touching nuclei are cut apart.
    Nuclei,
    /// Whole cells stained throughout.
    Cells,
    /// Small bright spots (puncta, foci).
    Spots,
}

/// Bright or dark objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PolarityArg {
    /// Objects brighter than the background (fluorescence).
    Bright,
    /// Objects darker than the background (brightfield).
    Dark,
}

/// How the `--objects` threshold is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MethodArg {
    /// Otsu's, or the triangle method when objects are sparse.
    Auto,
    /// Otsu's.
    Otsu,
    /// The triangle method.
    Triangle,
    /// Each pixel against its neighbourhood.
    Local,
}

/// What `--objects` are found in on an RGB image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StainArg {
    /// Hematoxylin (colour deconvolution).
    Hematoxylin,
    /// Eosin (colour deconvolution).
    Eosin,
    /// DAB (colour deconvolution).
    Dab,
    /// The red component.
    Red,
    /// The green component.
    Green,
    /// The blue component.
    Blue,
    /// The mean of the components.
    Luminance,
}

/// The measurement `--line`, `--polygon` and `--objects` ask for; `None` without them.
fn measure_request(
    a: &StatsArgs,
    region: Option<openreadout_core::Region>,
) -> Result<Option<openreadout_core::measure::MeasureRequest>> {
    use openreadout_core::Error;
    use openreadout_core::measure::{
        Kind, MeasureRequest, ObjectsRequest, Polarity, Stain, ThresholdMethod,
    };
    let nums = |s: &str, what: &str| -> Result<Vec<f64>> {
        s.split([',', ' ', ';'])
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                p.trim()
                    .parse::<f64>()
                    .map_err(|_| Error::Usage(format!("{what} '{s}': '{p}' is not a number")))
            })
            .collect()
    };
    let line = match a.line.as_deref() {
        None => None,
        Some(s) => match nums(s, "--line")?[..] {
            [x0, y0, x1, y1] => Some([x0, y0, x1, y1]),
            _ => {
                return Err(Error::Usage(format!(
                    "--line '{s}': expected X0,Y0,X1,Y1 in pixels"
                )));
            }
        },
    };
    let polygon = match a.polygon.as_deref() {
        None => None,
        Some(s) => {
            let v = nums(s, "--polygon")?;
            if v.len() < 6 || v.len() % 2 != 0 {
                return Err(Error::Usage(format!(
                    "--polygon '{s}': expected at least three X,Y vertices separated by spaces"
                )));
            }
            Some(v.as_chunks::<2>().0.to_vec())
        }
    };
    let objects = a.objects.map(|k| ObjectsRequest {
        kind: match k {
            KindArg::Nuclei => Kind::Nuclei,
            KindArg::Cells => Kind::Cells,
            KindArg::Spots => Kind::Spots,
        },
        diameter_um: a.diameter,
        diameter_px: a.diameter_px,
        threshold: a.threshold,
        method: match a.method {
            None | Some(MethodArg::Auto) => ThresholdMethod::Auto,
            Some(MethodArg::Otsu) => ThresholdMethod::Otsu,
            Some(MethodArg::Triangle) => ThresholdMethod::Triangle,
            Some(MethodArg::Local) => ThresholdMethod::Local,
        },
        polarity: match a.polarity {
            None => Polarity::Auto,
            Some(PolarityArg::Bright) => Polarity::Bright,
            Some(PolarityArg::Dark) => Polarity::Dark,
        },
        stain: match a.stain {
            None => Stain::Auto,
            Some(StainArg::Hematoxylin) => Stain::Hematoxylin,
            Some(StainArg::Eosin) => Stain::Eosin,
            Some(StainArg::Dab) => Stain::Dab,
            Some(StainArg::Red) => Stain::Red,
            Some(StainArg::Green) => Stain::Green,
            Some(StainArg::Blue) => Stain::Blue,
            Some(StainArg::Luminance) => Stain::Luminance,
        },
        split: a.no_split.then_some(false),
        min_area_um2: a.min_area,
        max_area_um2: a.max_area,
        max_objects: a.max_objects,
        outlines: false,
    });
    let req = MeasureRequest {
        image: a.image,
        select: a.select.clone(),
        level: a.level,
        region,
        line,
        line_width: a.line_width,
        polygon,
        objects,
    };
    Ok(req.any().then_some(req))
}

fn render_measure(o: &openreadout_core::measure::MeasureOutput) -> String {
    let mut out = Vec::new();
    let um =
        |v: Option<f64>, unit: &str| v.map(|v| format!(" ({v:.3} {unit})")).unwrap_or_default();
    let name = |n: &Option<String>| n.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
    if let Some(p) = &o.profile {
        out.push(format!(
            "profile {:.1} px{}, {} samples",
            p.length_px,
            um(p.length_um, "µm"),
            p.samples
        ));
        for c in &p.channels {
            out.push(format!(
                "  c={}{}  min {}  max {} at {:.1} px{}  mean {}{}",
                c.c,
                name(&c.name),
                num(Some(c.min)),
                num(Some(c.max)),
                c.max_at_px,
                um(c.max_at_um, "µm"),
                num(Some(c.mean)),
                c.fwhm_px
                    .map(|w| format!("  FWHM {w:.1} px{}", um(c.fwhm_um, "µm")))
                    .unwrap_or_default()
            ));
        }
    }
    if let Some(r) = &o.roi {
        out.push(format!("polygon {} px{}", r.area_px, um(r.area_um2, "µm²")));
        for c in &r.channels {
            out.push(format!(
                "  c={}{}  mean {}  std {}  median {}  min {}  max {}  sum {}",
                c.c,
                name(&c.name),
                num(Some(c.mean)),
                num(Some(c.std)),
                num(Some(c.median)),
                num(Some(c.min)),
                num(Some(c.max)),
                num(Some(c.sum))
            ));
        }
    }
    if let Some(ob) = &o.objects {
        out.push(render_objects(ob));
    }
    for n in &o.notes {
        out.push(format!("note: {n}"));
    }
    out.join("\n")
}

fn render_objects(ob: &openreadout_core::measure::Objects) -> String {
    let mut out = Vec::new();
    let kind = serde_json::to_value(ob.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    out.push(format!(
        "{} {kind} in {} ({}; {} touching the edge{})",
        ob.count,
        ob.region,
        ob.measured,
        ob.touching_edge,
        ob.density_per_mm2
            .map(|d| format!(", {} per mm²", num(Some(d))))
            .unwrap_or_default()
    ));
    out.push(format!(
        "  threshold {} {}, expected diameter {} px{} ({})",
        num(Some(ob.threshold)),
        ob.threshold_method,
        num(Some(ob.expected_diameter_px)),
        ob.expected_diameter_um
            .map(|d| format!(" = {} µm", num(Some(d))))
            .unwrap_or_default(),
        ob.expected_diameter_from
    ));
    if let Some(a) = &ob.area_um2 {
        out.push(format!(
            "  area µm²: median {}  min {}  max {}",
            num(Some(a.median)),
            num(Some(a.min)),
            num(Some(a.max))
        ));
    } else if let Some(a) = &ob.area_px {
        out.push(format!(
            "  area px: median {}  min {}  max {}",
            num(Some(a.median)),
            num(Some(a.min)),
            num(Some(a.max))
        ));
    }
    if let Some(d) = &ob.diameter_um {
        out.push(format!("  diameter µm: median {}", num(Some(d.median))));
    }
    for c in &ob.channels {
        let pos = c
            .positive
            .as_ref()
            .filter(|p| p.method != "none")
            .map(|p| {
                format!(
                    "  positive {} ({:.0}%) above {} ({})",
                    p.count,
                    p.fraction * 100.0,
                    num(Some(p.threshold)),
                    p.method
                )
            })
            .unwrap_or_default();
        out.push(format!(
            "  c={}{}  object mean median {}  background {}{pos}",
            c.c,
            c.name
                .as_deref()
                .map(|n| format!(" {n}"))
                .unwrap_or_default(),
            num(c.mean.as_ref().map(|m| m.median)),
            num(Some(c.background))
        ));
    }
    if ob.large > 0 {
        out.push(format!(
            "  {} objects over twice the median area (possibly several together)",
            ob.large
        ));
    }
    out.join("\n")
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
    match measure_request(a, region) {
        Err(e) => return crate::output::fail(a.json, &e),
        Ok(Some(m)) => {
            return batch::run(
                reg,
                &a.files,
                Spec {
                    json: a.json,
                    batch: &a.batch,
                    stdin: Stdin::Spool,
                },
                &mut |input| {
                    let (_, mut ds) = reg.open(input.path)?;
                    let info = ds.info()?;
                    openreadout_core::measure::measure(ds.as_mut(), &info, &m)
                },
                &render_measure,
            );
        }
        Ok(None) => {}
    }
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
    if !o.images_skipped.is_empty() {
        let list: Vec<String> = o.images_skipped.iter().map(u32::to_string).collect();
        s.push_str(&format!(
            "\n(left out: image(s) {}, whose pixel data the reader does not decode; `info` says why)",
            list.join(", ")
        ));
    }
    s.trim_end().to_string()
}
