//! `stats --tidy`: pixel statistics, one row per data set × image × channel (or per image, or
//! per plane).

use openreadout_core::parallel::ReadContext;
use openreadout_core::stats::{Projection, SampleStats, StatsRequest, compute_stats};
use openreadout_core::{Dataset, Result};

use super::image_well;
use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};

/// Row grain of `stats --tidy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatsPer {
    /// One row per image and channel (every selected plane of the channel).
    #[default]
    Channel,
    /// One row per image (every selected plane).
    Image,
    /// One row per plane.
    Plane,
}

/// Pixel statistics.
#[derive(Debug, Clone, Default)]
pub struct StatsMeasure {
    /// Only this image.
    pub image: Option<u32>,
    /// Plane selection (`c=0`, `z=2-5`).
    pub select: Vec<String>,
    /// Pyramid level.
    pub level: u32,
    /// Row grain.
    pub per: StatsPer,
    /// Maximum-intensity projection along z or t first (`stats --mip`): the rows are the
    /// statistics of the projections.
    pub mip: Option<Projection>,
}

fn put_stats(r: &mut Row, s: &SampleStats) {
    let p = s.percentiles.as_ref();
    r.set("count", s.count);
    r.set("min", Value::float_opt(s.min));
    r.set("max", Value::float_opt(s.max));
    r.set("mean", Value::float_opt(s.mean));
    r.set("std", Value::float_opt(s.std));
    r.set("median", Value::float_opt(p.map(|p| p.p50)));
    r.set("p1", Value::float_opt(p.map(|p| p.p1)));
    r.set("p5", Value::float_opt(p.map(|p| p.p5)));
    r.set("p95", Value::float_opt(p.map(|p| p.p95)));
    r.set("p99", Value::float_opt(p.map(|p| p.p99)));
    r.set("zero_fraction", s.zero_fraction);
    r.set("saturated_fraction", Value::float_opt(s.saturated_fraction));
    r.set("saturated_count", s.saturated_count);
    r.set("exact", s.exact);
}

impl Measure for StatsMeasure {
    fn id(&self) -> &'static str {
        "stats"
    }
    fn grain(&self) -> Vec<String> {
        let mut g = vec!["image".to_string()];
        match self.per {
            StatsPer::Image => {}
            StatsPer::Channel => g.push("channel".into()),
            StatsPer::Plane => g.extend(["channel".into(), "z".into(), "t".into()]),
        }
        g
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        vec![
            ColumnDoc::key("image", "image (scene, series, position) index, from 0"),
            ColumnDoc::key("image_name", "image name as recorded"),
            ColumnDoc::key("well", "plate well of the image (HCS scenes), as A01"),
            ColumnDoc::key("channel", "channel index, from 0"),
            ColumnDoc::key("channel_name", "channel name as recorded"),
            ColumnDoc::key("z", "z index"),
            ColumnDoc::key("t", "time index"),
            ColumnDoc::value("planes", "planes aggregated"),
            ColumnDoc::value(
                "mip",
                "with --mip: the axis projected (z or t); the statistics are of the maximum-intensity projections",
            ),
            ColumnDoc::value("pixel_type", "sample type (uint8, uint16, float, …)"),
            ColumnDoc::value("count", "finite samples"),
            ColumnDoc::value("min", "smallest value (raw stored units)"),
            ColumnDoc::value("max", "largest value"),
            ColumnDoc::value("mean", "mean"),
            ColumnDoc::value("std", "population standard deviation"),
            ColumnDoc::value("median", "50th percentile (NumPy linear interpolation)"),
            ColumnDoc::value("p1", "1st percentile"),
            ColumnDoc::value("p5", "5th percentile"),
            ColumnDoc::value("p95", "95th percentile"),
            ColumnDoc::value("p99", "99th percentile"),
            ColumnDoc::value("zero_fraction", "fraction of samples equal to 0"),
            ColumnDoc::value(
                "saturated_fraction",
                "fraction at the saturation level: the recorded bit depth's ceiling, else the type's maximum (integer types)",
            ),
            ColumnDoc::value(
                "saturated_count",
                "samples at the saturation level (integer types)",
            ),
            ColumnDoc::value("red_mean", "RGB images: mean of the red samples"),
            ColumnDoc::value("green_mean", "RGB images: mean of the green samples"),
            ColumnDoc::value("blue_mean", "RGB images: mean of the blue samples"),
            ColumnDoc::value(
                "exact",
                "percentiles exact (false: within 0.2 % for data with > 131,072 distinct values)",
            ),
        ]
    }
    fn fingerprint(&self) -> String {
        format!(
            "stats:{:?}:{:?}:{}:{:?}{}",
            self.image,
            self.select,
            self.level,
            self.per,
            self.mip.map(|m| format!(":mip={m:?}")).unwrap_or_default()
        )
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        if it.info.images.is_empty() {
            return Err(super::not_applicable(
                "stats",
                it.info,
                "images",
                "Pixel statistics need images; use `trace --tidy` for signals and `table --tidy` for tables.",
            ));
        }
        let req = {
            let mut r = StatsRequest::default();
            r.image = self.image;
            r.select = self.select.clone();
            r.level = self.level;
            r.bins = 0;
            r.per_plane = self.per == StatsPer::Plane;
            r.mip = self.mip;
            r
        };
        let mip_axis = self.mip.map(Projection::name);
        let reg = it.registry;
        let path = it.path;
        let opener = || -> Result<Box<dyn Dataset>> { reg.open(path).map(|(_, d)| d) };
        let out = compute_stats(
            it.dataset,
            it.info,
            &req,
            &ReadContext {
                opener: Some(&opener),
                progress: None,
            },
        )?;
        let image_base = |i: u32| {
            let im = it.info.images.iter().find(|x| x.index == i);
            let mut r = Row::new().with("image", i);
            if let Some(n) = im.and_then(|x| x.name.as_deref()) {
                r.set("image_name", n);
            }
            if let Some(w) = im.and_then(image_well) {
                r.set("well", w);
            }
            if let Some(a) = mip_axis {
                r.set("mip", a);
            }
            r
        };
        let channel_name = |i: u32, c: u32| {
            it.info
                .images
                .iter()
                .find(|x| x.index == i)
                .and_then(|x| x.channels.get(c as usize))
                .and_then(|ch| ch.name.clone())
        };
        let mut rows = Vec::new();
        match self.per {
            StatsPer::Image => {
                for im in &out.images {
                    let mut r = image_base(im.image);
                    r.set("planes", im.planes);
                    r.set("pixel_type", im.pixel_type.ome_name());
                    put_stats(&mut r, &im.stats);
                    rows.push(r);
                }
            }
            StatsPer::Channel => {
                for ch in &out.channels {
                    let mut r = image_base(ch.image);
                    r.set("channel", ch.c);
                    r.set("channel_name", Value::text_opt(ch.name.as_deref()));
                    r.set("planes", ch.planes);
                    if let Some(im) = out.images.iter().find(|i| i.image == ch.image) {
                        r.set("pixel_type", im.pixel_type.ome_name());
                    }
                    put_stats(&mut r, &ch.stats);
                    for comp in &ch.components {
                        if let Some(n) = comp.name.as_deref().filter(|n| *n != "alpha") {
                            r.set(format!("{n}_mean"), Value::float_opt(comp.stats.mean));
                        }
                    }
                    rows.push(r);
                }
            }
            StatsPer::Plane => {
                for p in &out.planes {
                    let mut r = image_base(p.image);
                    r.set("channel", p.c);
                    r.set(
                        "channel_name",
                        Value::text_opt(channel_name(p.image, p.c).as_deref()),
                    );
                    r.set("z", p.z);
                    r.set("t", p.t);
                    r.set("planes", 1u32);
                    r.set("pixel_type", p.pixel_type.ome_name());
                    put_stats(&mut r, &p.stats);
                    rows.push(r);
                }
            }
        }
        Ok(rows)
    }
}
