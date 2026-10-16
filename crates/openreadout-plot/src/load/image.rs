//! Intensity profiles: the values of an image plane along a line (`show: profile`), sampled
//! once per pixel with bilinear interpolation, against the distance along the line in mm (µm
//! below 1 mm) when the pixel size is known, else in pixels.

use openreadout_core::Result;
use openreadout_core::reader::PlaneIndex;

use super::{Ctx, usage};
use crate::model::{Geom, Quantity, Series, Source};
use crate::spec::{MarkKind, SeriesSpec};

/// Most samples along one line.
const MAX_SAMPLES: usize = 8192;

pub(crate) fn profile(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let line = spec.line.ok_or_else(|| {
        usage("show profile needs `line`: [[x0, y0], [x1, y1]] in the image's full-resolution pixels (the preview's ruler numbers)")
    })?;
    let (_, mut ds, info) = ctx.open(file)?;
    let index = spec.image.unwrap_or(0);
    let im = info
        .images
        .iter()
        .find(|i| i.index == index)
        .ok_or_else(|| {
            usage(format!(
                "{file}: image {index} not found ({} images)",
                info.images.len()
            ))
        })?;
    let sel = openreadout_core::select::Selection::parse(
        &spec.select.iter().cloned().collect::<Vec<_>>(),
    )?;
    let c = spec.channel.or(sel.c.first().copied()).unwrap_or(0);
    let z = sel.z.first().copied().unwrap_or(im.size_z / 2);
    let t = sel.t.first().copied().unwrap_or(0);
    if c >= im.size_c || z >= im.size_z || t >= im.size_t {
        return Err(usage(format!(
            "plane c={c} z={z} t={t} is out of range (c<{}, z<{}, t<{})",
            im.size_c, im.size_z, im.size_t
        )));
    }
    let [[x0, y0], [x1, y1]] = line;
    for v in [x0, y0, x1, y1] {
        if !v.is_finite() {
            return Err(usage("line coordinates must be numbers"));
        }
    }
    let (w, h) = (f64::from(im.size_x), f64::from(im.size_y));
    // the image's edges (0 and the width or height) are inside, as for `stats --line`
    let inside = |x: f64, y: f64| x >= 0.0 && y >= 0.0 && x <= w && y <= h;
    if !inside(x0, y0) || !inside(x1, y1) {
        return Err(usage(format!(
            "line ({x0}, {y0}) to ({x1}, {y1}) leaves the {}x{} image",
            im.size_x, im.size_y
        )));
    }
    let len_px = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    let n = ((len_px.ceil() as usize) + 1).clamp(2, MAX_SAMPLES);
    ctx.spend(n)?;
    let plane = ds.read_plane(index, PlaneIndex { c, z, t })?;
    let spp = plane.samples_per_pixel.max(1) as usize;
    let vals: Vec<f64> = plane.pixel_type.samples_f64(&plane.data).collect();
    let pw = plane.width as usize;
    let at = |x: usize, y: usize| -> f64 {
        // the first sample (red of RGB) of the pixel
        vals.get((y * pw + x) * spp).copied().unwrap_or(f64::NAN)
    };
    // physical step along the line
    let (sx, sy) = (im.physical_size.x, im.physical_size.y);
    let um_len = match (sx, sy) {
        (Some(a), Some(b)) => Some((((x1 - x0) * a).powi(2) + ((y1 - y0) * b).powi(2)).sqrt()),
        _ => None,
    };
    let (unit, scale) = match um_len {
        Some(l) if l >= 1000.0 => ("mm", l / 1000.0),
        Some(l) => ("µm", l),
        None => ("px", len_px),
    };
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let f = i as f64 / (n - 1) as f64;
        let x = (x0 + (x1 - x0) * f).min(w - 1.0);
        let y = (y0 + (y1 - y0) * f).min(h - 1.0);
        let (xa, ya) = (x.floor() as usize, y.floor() as usize);
        let (xb, yb) = (
            (xa + 1).min(pw - 1),
            (ya + 1).min(plane.height as usize - 1),
        );
        let (tx, ty) = (x - x.floor(), y - y.floor());
        let v = (at(xa, ya) * (1.0 - tx) + at(xb, ya) * tx) * (1.0 - ty)
            + (at(xa, yb) * (1.0 - tx) + at(xb, yb) * tx) * ty;
        pts.push((f * scale, v));
    }
    let value_unit = im
        .extra
        .get("value_unit")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let yname = if value_unit.as_deref() == Some("HU") {
        "CT number"
    } else {
        "intensity"
    };
    let channel_name = im
        .channels
        .iter()
        .find(|ch| ch.index == c)
        .and_then(|ch| ch.name.clone());
    let name = channel_name.unwrap_or_else(|| format!("c={c}"));
    let s = Series::new(
        name,
        Geom::Line(pts),
        MarkKind::Line,
        Source::file(
            file,
            format!("image {index} c={c} z={z} t={t}, line ({x0}, {y0}) to ({x1}, {y1}) px"),
        ),
    )
    .axes(
        Quantity::new("distance along the line", Some(unit)),
        Quantity::new(yname, value_unit.as_deref()),
    );
    if spp > 1 {
        ctx.notes
            .push("the profile of an RGB image follows its red samples".into());
    }
    Ok(vec![s])
}
