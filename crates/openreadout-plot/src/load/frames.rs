//! `show: frames`: one value per frame (time point) of an image, against the frame number or
//! a value the frame records: the dose of each frame of an electron-event movie, the angle of
//! each image of a rotation sweep, or a pixel statistic of every frame (`mean`, `sum`, `max`,
//! `min`).

use openreadout_core::{PlaneIndex, Result};
use serde_json::Value;

use super::{Ctx, usage};
use crate::model::{Geom, Quantity, Series, Source};
use crate::spec::{MarkKind, SeriesSpec};

/// Pixel statistics a frame series can show.
const STATS: &[&str] = &["mean", "sum", "max", "min"];
/// Units our frame-record names end in.
const UNIT_SUFFIXES: &[(&str, &str)] = &[
    ("_e_per_a2", "e/Å²"),
    ("_deg", "°"),
    ("_um", "µm"),
    ("_s", "s"),
    ("_a", "Å"),
];
/// Frames read for pixel statistics at most.
const MAX_FRAMES: usize = 20_000;

pub(super) fn frames(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let (_, mut ds, info) = ctx.open(file)?;
    // A file of metadata only (a SerialEM .mdoc) has its records as the rows of its first
    // table: one per section.
    let from_table = info.images.is_empty() && !info.tables.is_empty();
    let fallback =
        openreadout_core::model::ImageInfo::new(0, 1, 1, openreadout_core::PixelType::Uint8);
    let im = match info.images.first() {
        Some(im) => im,
        None if from_table => &fallback,
        None => {
            return Err(usage(format!(
                "{file}: no image; frames are the time points of an image"
            )));
        }
    };
    let c = spec.channel.unwrap_or(0);
    if c >= im.size_c {
        return Err(usage(format!(
            "{file}: channel {c} out of range (the image has {})",
            im.size_c
        )));
    }
    // Frames are the time points, or the sections of a z stack (a tilt series saved as a
    // volume) when there is one time point.
    let along_z = im.size_t <= 1 && im.size_z > 1;
    let (n, records) = if from_table {
        let t = &info.tables[0];
        let rows = ds.read_table(t.index, 0, t.row_count)?;
        let n = usize::try_from(t.row_count).unwrap_or(usize::MAX);
        let records: Vec<Value> = (0..n)
            .map(|i| {
                Value::Object(
                    t.columns
                        .iter()
                        .zip(&rows.columns)
                        .filter_map(|(col, vals)| {
                            let v = *vals.get(i)?;
                            v.is_finite()
                                .then(|| (col.name.clone(), serde_json::json!(v)))
                        })
                        .collect(),
                )
            })
            .collect();
        (n, records)
    } else {
        let n = if along_z { im.size_z } else { im.size_t.max(1) } as usize;
        (n, ds.frames(im.index, None)?.1)
    };
    // The fields the frames record, in the order the first record has them.
    let fields: Vec<String> = records
        .first()
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter(|(_, v)| v.is_number())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let y = spec.y.clone().unwrap_or_else(|| {
        if fields.iter().any(|f| f == "dose") {
            "dose".into()
        } else {
            "mean".into()
        }
    });
    let x = spec.x.clone().unwrap_or_else(|| "t".into());
    let known =
        |name: &str| name == "t" || STATS.contains(&name) || fields.iter().any(|f| f == name);
    for (axis, name) in [("x", &x), ("y", &y)] {
        if !known(name) {
            return Err(usage(format!(
                "{file}: frames have no `{name}` for {axis}; use t (the frame number), {} or a value the frames record ({})",
                STATS.join(", "),
                if fields.is_empty() {
                    "none here".to_string()
                } else {
                    fields.join(", ")
                }
            )));
        }
    }
    let field = |name: &str, i: usize| -> Option<f64> {
        if name == "t" {
            return Some(i as f64);
        }
        records.get(i)?.get(name)?.as_f64()
    };
    let needs_pixels = STATS.contains(&x.as_str()) || STATS.contains(&y.as_str());
    if needs_pixels && from_table {
        return Err(usage(format!(
            "{file}: no image to take pixel statistics from; plot values the sections record"
        )));
    }
    if needs_pixels && n > MAX_FRAMES {
        return Err(usage(format!(
            "{file}: {n} frames; pixel statistics are read for at most {MAX_FRAMES}"
        )));
    }
    ctx.spend(n)?;
    let mut stats: Vec<[f64; 4]> = Vec::new();
    if needs_pixels {
        for t in 0..n {
            let p = ds.read_plane(
                im.index,
                if along_z {
                    PlaneIndex {
                        c,
                        z: t as u32,
                        t: 0,
                    }
                } else {
                    PlaneIndex {
                        c,
                        z: im.size_z / 2,
                        t: t as u32,
                    }
                },
            )?;
            let bps = p.pixel_type.bytes_per_sample();
            let (mut sum, mut mx, mut mn, mut k) = (0f64, f64::NEG_INFINITY, f64::INFINITY, 0u64);
            for s in p.data.chunks_exact(bps) {
                let v = p.pixel_type.sample_f64(s);
                if v.is_finite() {
                    sum += v;
                    mx = mx.max(v);
                    mn = mn.min(v);
                    k += 1;
                }
            }
            let mean = if k > 0 { sum / k as f64 } else { f64::NAN };
            stats.push([mean, sum, mx, mn]);
        }
        ctx.notes.push(format!(
            "{file}: read every frame ({n}) for its pixel statistics (as stored: no mask or gain applied)"
        ));
    }
    let value = |name: &str, i: usize| -> Option<f64> {
        match STATS.iter().position(|s| *s == name) {
            Some(k) => stats.get(i).map(|s| s[k]),
            None => field(name, i),
        }
    };
    let mut pts: Vec<(f64, f64)> = (0..n)
        .filter_map(|i| Some((value(&x, i)?, value(&y, i)?)))
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .collect();
    if let Some([lo, hi]) = spec.x_range {
        pts.retain(|(a, _)| *a >= lo && *a <= hi);
    }
    if pts.is_empty() {
        return Err(usage(format!("{file}: no frame has both {x} and {y}")));
    }
    let unit_of = |name: &str| -> Option<String> {
        for (suffix, unit) in UNIT_SUFFIXES {
            if name.ends_with(suffix) {
                return Some((*unit).into());
            }
        }
        records
            .first()
            .and_then(|r| r.get("units"))
            .and_then(|u| u.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let label = |name: &str| -> String {
        match name {
            "t" => "frame".into(),
            "dose" => "dose per frame".into(),
            other => UNIT_SUFFIXES
                .iter()
                .find_map(|(suffix, _)| other.strip_suffix(suffix))
                .unwrap_or(other)
                .to_string(),
        }
    };
    let mark = if pts.len() > 200 {
        MarkKind::Line
    } else {
        MarkKind::LinePoints
    };
    let mut s = Series::new(
        spec.label.clone().unwrap_or_else(|| label(&y)),
        Geom::Line(pts),
        mark,
        Source {
            file: Some(file.to_string()),
            selector: format!("frames of image {}: {y} against {x}", im.index),
        },
    );
    s.x = Quantity::new(label(&x), unit_of(&x).as_deref());
    s.y = Quantity::new(label(&y), unit_of(&y).as_deref());
    Ok(vec![s])
}
