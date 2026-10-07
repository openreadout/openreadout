//! `check` (integrity) and `planes` (plane hashes). Comparing two files is `compare`
//! (`compare.rs`), the diagnostic bundle is `report` (`report.rs`).

use std::path::{Path, PathBuf};

use openreadout_core::model::{CheckReport, PlaneHash, PlanesOutput};
use openreadout_core::reader::PlaneIndex;
use openreadout_core::{Error, Registry, Result};

use super::batch::{self, BatchArgs, Spec, Stdin};
use super::{live_err, render_assurance_line};

/// Arguments of `check`.
#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    /// Files, directories or glob patterns; several make a batch (see `--recursive`,
    /// `--jsonl`). `-` reads standard input.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    /// Check headers and structure only (offsets, declared sizes and counts, missing parts):
    /// no decompression, no decoding, no checksums over the data. What `index` runs on every
    /// data set.
    #[arg(long)]
    pub headers_only: bool,
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `planes`.
#[derive(Debug, clap::Args)]
pub struct PlanesArgs {
    /// Files, directories or glob patterns; several make a batch (see `--recursive`,
    /// `--jsonl`). `-` reads standard input.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    #[arg(long)]
    pub json: bool,
    /// Only this image index.
    #[arg(long)]
    pub image: Option<u32>,
    /// Plane selection, e.g. `c=0`, `z=2-5`, `t=0,3`. Repeatable.
    #[arg(long = "select")]
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution).
    #[arg(long, default_value_t = 0)]
    pub level: u32,
    /// Only this rectangle of each plane, `X,Y,WIDTH,HEIGHT` in the pixel coordinates of
    /// `--level`. Tiled readers decode only the tiles it touches.
    #[arg(long, value_name = "X,Y,W,H")]
    pub region: Option<String>,
    /// Also write each plane's raw little-endian samples to `DIR/image<i>_c<c>_z<z>_t<t>.bin`.
    #[arg(long, value_name = "DIR")]
    pub dump_dir: Option<PathBuf>,
}

/// Run `planes`.
pub fn run_planes(reg: &Registry, a: &PlanesArgs) -> i32 {
    let spec = Spec {
        json: a.json,
        batch: &a.batch,
        stdin: Stdin::Spool,
    };
    batch::run(
        reg,
        &a.files,
        spec,
        &mut |i| {
            live_err(i.path, || {
                let region = a
                    .region
                    .as_deref()
                    .map(openreadout_core::Region::parse)
                    .transpose()?;
                planes(
                    reg,
                    i,
                    a.image,
                    &a.select,
                    a.level,
                    region,
                    a.dump_dir.as_deref(),
                )
            })
        },
        &render_planes,
    )
}

/// Run `check`.
pub fn run(reg: &Registry, a: &CheckArgs) -> i32 {
    let spec = Spec {
        json: a.json,
        batch: &a.batch,
        stdin: Stdin::Spool,
    };
    batch::run(
        reg,
        &a.files,
        spec,
        &mut |i| live_err(i.path, || check(reg, i.path, a.headers_only)),
        &render_check,
    )
}

fn check(reg: &Registry, file: &Path, headers_only: bool) -> Result<CheckReport> {
    let (_, mut ds) = reg.open(file)?;
    let mut r = if headers_only {
        ds.check_headers()?
    } else {
        ds.check()?
    };
    if let Some(a) = openreadout_core::live::assess_dataset(ds.as_ref(), file) {
        openreadout_core::live::apply_to_check(&mut r, &a);
    }
    r.assurance = ds.file_assurance();
    Ok(r)
}

fn planes(
    reg: &Registry,
    input: &batch::Input<'_>,
    image: Option<u32>,
    select: &[String],
    level: u32,
    region: Option<openreadout_core::Region>,
    dump_dir: Option<&Path>,
) -> Result<PlanesOutput> {
    use openreadout_core::parallel::{PlaneRequest, ReadContext, read_in_order};
    let file = input.path;
    if let Some(d) = dump_dir {
        std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
    }
    let (det, mut ds) = reg.open(file)?;
    let info = ds.info()?;
    let sel = openreadout_core::select::Selection::parse(select)?;
    let ws = ds.write_state();
    let acquisition = ws
        .as_ref()
        .and_then(|w| openreadout_core::live::assess(file, w));
    // While the file is still being written, read only the planes already complete.
    let only = ws
        .as_ref()
        .filter(|_| {
            acquisition
                .as_ref()
                .is_some_and(openreadout_core::live::Acquisition::in_progress)
        })
        .map(openreadout_core::live::complete_set);
    let mut out = PlanesOutput {
        path: file.display().to_string(),
        format: det.format_id.into(),
        planes: Vec::new(),
        acquisition,
    };
    let mut requests = Vec::new();
    let mut plane_bytes = 0u64;
    for im in &info.images {
        if image.is_some_and(|i| i != im.index) {
            continue;
        }
        let (w, h) = match region {
            Some(r) => (r.width, r.height),
            None => {
                openreadout_core::region::level_size(im, level).unwrap_or((im.size_x, im.size_y))
            }
        };
        plane_bytes = plane_bytes.max(
            u64::from(w)
                * u64::from(h)
                * u64::from(im.samples_per_pixel.max(1))
                * im.pixel_type.bytes_per_sample() as u64,
        );
        for c in 0..im.size_c {
            for z in 0..im.size_z {
                for t in 0..im.size_t {
                    let done = only
                        .as_ref()
                        .is_none_or(|s| s.contains(&(im.index, PlaneIndex { c, z, t })));
                    if done && sel.contains(c, z, t) {
                        requests.push(PlaneRequest {
                            image: im.index,
                            index: PlaneIndex { c, z, t },
                            level,
                            region,
                        });
                    }
                }
            }
        }
    }
    let opener =
        || -> Result<Box<dyn openreadout_core::Dataset>> { reg.open(file).map(|(_, d)| d) };
    let bar: std::sync::OnceLock<crate::ui::Progress> = std::sync::OnceLock::new();
    let total = requests.len() as u64;
    let keep = dump_dir.is_some();
    read_in_order(
        ds.as_mut(),
        &ReadContext {
            opener: Some(&opener),
            progress: None,
        },
        &requests,
        plane_bytes,
        &|_, p| {
            let h = p.xxh3_hex();
            let data = if keep { p.data } else { Vec::new() };
            Ok((
                p.width,
                p.height,
                p.pixel_type,
                p.samples_per_pixel,
                h,
                data,
            ))
        },
        &mut |i, (width, height, pixel_type, samples_per_pixel, xxh3, data)| {
            let r = &requests[i];
            let PlaneIndex { c, z, t } = r.index;
            if let Some(d) = dump_dir {
                let lv = if level > 0 {
                    format!("_level{level}")
                } else {
                    String::new()
                };
                let rg = region.map_or_else(String::new, |g| {
                    format!("_x{}_y{}_w{}_h{}", g.x, g.y, g.width, g.height)
                });
                let f = d.join(format!("image{}{lv}{rg}_c{c}_z{z}_t{t}.bin", r.image));
                std::fs::write(&f, &data).map_err(|e| Error::io(&f, e))?;
            }
            out.planes.push(PlaneHash {
                image: r.image,
                level,
                region,
                c,
                z,
                t,
                width,
                height,
                pixel_type,
                samples_per_pixel,
                xxh3,
            });
            if !input.batch {
                bar.get_or_init(|| crate::ui::Progress::new(total, "planes"))
                    .set(i as u64 + 1, None);
            }
            Ok(())
        },
    )?;
    Ok(out)
}

fn render_planes(p: &PlanesOutput) -> String {
    let mut s = format!("{} ({}): {} planes\n", p.path, p.format, p.planes.len());
    for h in &p.planes {
        s.push_str(&format!(
            "  image {}{} c={} z={} t={}  {}x{} {}  {}\n",
            h.image,
            if h.level > 0 {
                format!(" level {}", h.level)
            } else {
                String::new()
            },
            h.c,
            h.z,
            h.t,
            h.width,
            h.height,
            h.pixel_type.ome_name(),
            h.xxh3
        ));
    }
    s.trim_end().to_string()
}

fn render_check(r: &CheckReport) -> String {
    let mut s = format!(
        "{} ({}): {}\n",
        r.path,
        r.format,
        if r.ok {
            crate::ui::paint(crate::ui::OK, "OK")
        } else {
            crate::ui::paint(crate::ui::ERR, "PROBLEMS FOUND")
        }
    );
    for f in &r.findings {
        let sev = f.severity.as_str();
        s.push_str(&format!(
            "  {sev:<8} {:<18} {}{}\n",
            f.code,
            f.message,
            f.offset.map(|o| format!(" @{o}")).unwrap_or_default()
        ));
    }
    if let Some(a) = &r.assurance {
        s.push_str(&render_assurance_line(a));
    }
    s.push_str("  checks performed:\n");
    for c in &r.checks_performed {
        s.push_str(&format!("    - {c}\n"));
    }
    s.trim_end().to_string()
}
