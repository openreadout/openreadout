//! `compare A B`: metadata diff, geometry, channel names, physical sizes and plane hashes
//! of two files (`openreadout_ops::compare`). Exit 0 when identical, 1 when they differ.

use std::path::PathBuf;

use openreadout_core::{Registry, Result};
use openreadout_ops::compare::{CompareOutput, CompareRequest, compare};

use crate::output::{emit, fail};
use crate::ui;

/// Arguments of `compare`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct CompareArgs {
    /// The first file (e.g. the raw file).
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// The second file (e.g. its OME-TIFF export).
    #[arg(value_name = "AGAINST")]
    pub against: PathBuf,
    /// Only this image index (in both files).
    #[arg(long)]
    pub image: Option<u32>,
    /// Plane selection, e.g. `c=0`, `z=2-5`, `t=0,3`. Repeatable. A second file holding only
    /// the selected planes (an export with the same `--select`) is matched to them in order.
    #[arg(long = "select")]
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution).
    #[arg(long, default_value_t = 0)]
    pub level: u32,
    /// Largest absolute sample difference that still counts as equal (lossy conversions).
    /// Default: planes must be bit-identical (same xxh3-128).
    #[arg(long, value_name = "T")]
    pub tolerance: Option<f64>,
    /// Leave this JSON pointer (into `info --json` data) out of the metadata diff; `*` matches
    /// one segment, e.g. `/images/*/name`. Repeatable. `/path`, `/size_bytes`, `/format`,
    /// `/format_version`, `/notes` and `/images/*/dimension_order` are always left out.
    #[arg(long, value_name = "POINTER")]
    pub ignore: Vec<String>,
    /// Also diff the `extra` objects (format-specific; they usually differ between formats).
    #[arg(long)]
    pub include_extra: bool,
    /// Compare data only: skip the metadata diff.
    #[arg(long, conflicts_with_all = ["ignore", "include_extra"])]
    pub no_metadata: bool,
    /// Compare metadata and geometry only: do not read pixels.
    #[arg(long, conflicts_with = "tolerance")]
    pub no_pixels: bool,
    #[arg(long)]
    pub json: bool,
}

fn go(reg: &Registry, a: &CompareArgs) -> Result<CompareOutput> {
    let (_, mut da) = reg.open(&a.file)?;
    let (_, mut db) = reg.open(&a.against)?;
    let (ia, ib) = (da.info()?, db.info()?);
    compare(da.as_mut(), &ia, db.as_mut(), &ib, &{
        let mut compare_request = CompareRequest::default();
        compare_request.image = a.image;
        compare_request.select = a.select.clone();
        compare_request.level = a.level;
        compare_request.tolerance = a.tolerance;
        compare_request.ignore = a.ignore.clone();
        compare_request.include_extra = a.include_extra;
        compare_request.no_metadata = a.no_metadata;
        compare_request.no_pixels = a.no_pixels;
        compare_request
    })
}

/// Run `compare`: exit 0 when the files hold the same data, 1 when they differ.
pub fn run(reg: &Registry, a: &CompareArgs) -> i32 {
    match go(reg, a) {
        Ok(out) => {
            emit(a.json, &out, render);
            i32::from(!out.identical)
        }
        Err(e) => fail(a.json, &e),
    }
}

fn verdict(ok: bool) -> String {
    if ok {
        ui::paint(ui::OK, "same")
    } else {
        ui::paint(ui::ERR, "DIFFERENT")
    }
}

fn short(v: Option<&serde_json::Value>) -> String {
    match v {
        None => ui::paint(ui::DIM, "(absent)"),
        Some(v) => {
            let s = v.to_string();
            if s.chars().count() > 60 {
                format!("{}…", s.chars().take(59).collect::<String>())
            } else {
                s
            }
        }
    }
}

pub fn render(o: &CompareOutput) -> String {
    let mut s = format!(
        "{} ({})\n{} ({})\n",
        o.ours.path, o.ours.format, o.theirs.path, o.theirs.format
    );
    s.push_str(&format!(
        "=> {}\n",
        if o.identical {
            ui::paint(ui::OK, "identical")
        } else {
            ui::paint(ui::ERR, "different")
        }
    ));
    if o.metadata.compared {
        s.push_str(&format!(
            "metadata: {} ({} differences)\n",
            verdict(o.metadata.equal),
            o.metadata.difference_count
        ));
        for d in o.metadata.differences.iter().take(20) {
            s.push_str(&format!(
                "  {}  ours {}  theirs {}\n",
                d.pointer,
                short(d.ours.as_ref()),
                short(d.theirs.as_ref())
            ));
        }
        if o.metadata.difference_count > 20 {
            s.push_str(&format!(
                "  … {} more (see --json)\n",
                o.metadata.difference_count - 20
            ));
        }
    } else {
        s.push_str("metadata: not compared\n");
    }
    for im in &o.images {
        s.push_str(&format!(
            "image {}: geometry {}, channel names {}, physical size {}\n",
            im.image,
            verdict(im.geometry_equal),
            verdict(im.channel_names_equal),
            verdict(im.physical_size_equal)
        ));
        if !im.geometry_equal {
            let g = |g: Option<&openreadout_ops::compare::Geometry>| {
                g.map_or_else(
                    || "(absent)".to_string(),
                    |g| {
                        format!(
                            "{}x{} z={} c={} t={} {} x{}",
                            g.size_x,
                            g.size_y,
                            g.size_z,
                            g.size_c,
                            g.size_t,
                            g.pixel_type.ome_name(),
                            g.samples_per_pixel
                        )
                    },
                )
            };
            s.push_str(&format!(
                "  ours {}\n  theirs {}\n",
                g(im.ours.as_ref()),
                g(im.theirs.as_ref())
            ));
        }
    }
    let p = &o.planes;
    if p.compared {
        s.push_str(&format!(
            "planes: {} compared, {} identical, {} within tolerance, {} mismatched{}\n",
            p.planes,
            p.identical,
            p.within_tolerance,
            p.mismatched,
            if p.skipped_images.is_empty() {
                String::new()
            } else {
                format!(
                    " (images {:?} skipped: geometry differs or missing)",
                    p.skipped_images
                )
            }
        ));
        for m in p.mismatches.iter().take(20) {
            s.push_str(&format!(
                "  image {} c={} z={} t={}  {} vs {}{}{}\n",
                m.image,
                m.c,
                m.z,
                m.t,
                m.ours_xxh3,
                m.theirs_xxh3,
                m.max_abs_diff
                    .map(|d| format!("  max |diff| {d}"))
                    .unwrap_or_default(),
                m.differing_samples
                    .map(|n| format!(", {n} samples differ"))
                    .unwrap_or_default()
            ));
        }
    } else {
        s.push_str("planes: not compared\n");
    }
    for n in &o.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s.trim_end().to_string()
}
