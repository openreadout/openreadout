//! `marks: objects` on image previews: count the nuclei, cells or spots in what the picture
//! shows (`openreadout_core::measure`, the same count as `stats` objects) and draw each
//! object's outline, its number and the count.

use openreadout_core::measure::{Kind, MeasureRequest, Objects, ObjectsRequest, measure};
use openreadout_core::{Dataset, FileInfo, Region, Result};
use openreadout_figure::{
    Anchor, Baseline, Color, Face, Figure, Layer, Path, Pill, Rect, Stroke, TextStyle,
};

use crate::{Frame, PreviewOutput, PreviewRequest};

/// Most pixels counted for a picture: larger views are counted at a coarser pyramid level.
const MAX_COUNT_PIXELS: u64 = 1 << 24;
/// Objects listed in the preview's JSON by default (`stats` lists them all).
const LISTED: usize = 20;
/// Numbers are drawn on the objects up to this many.
const MAX_NUMBERED: usize = 80;

/// With objects and no channel selected, a single-channel preview shows the channel the
/// objects are counted in (a DNA stain for nuclei), not channel 0, so the outlines sit on the
/// plane they were found in. Returns the request with that channel selected, or `None` when
/// nothing changes. Composites already include the channel.
pub(crate) fn show_counted_channel(
    info: &FileInfo,
    req: &PreviewRequest,
    notes: &mut Vec<String>,
) -> Option<PreviewRequest> {
    if req.marks != crate::Marks::Objects && req.objects.is_none() {
        return None;
    }
    if req.composite || req.select.iter().any(|s| s.contains('c')) {
        return None;
    }
    let im = info
        .images
        .iter()
        .find(|i| i.index == req.image.unwrap_or(0))?;
    if im.size_c < 2 || im.samples_per_pixel == 3 {
        return None;
    }
    let kind = req.objects.as_ref().map(|o| o.kind).unwrap_or_default();
    let c = openreadout_core::measure::default_channel(im, kind);
    let name = im
        .channels
        .iter()
        .find(|ch| ch.index == c)
        .and_then(|ch| ch.name.clone().or_else(|| ch.fluorophore.clone()))
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    notes.push(format!(
        "showing channel {c}{name}, where the objects were counted; select c=N for another"
    ));
    let mut r = req.clone();
    r.select.push(format!("c={c}"));
    Some(r)
}

/// Count objects in the area the preview shows and draw them over the image.
pub(crate) fn draw(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    frame: &Frame,
    fig: &mut Figure,
    out: &mut PreviewOutput,
) -> Result<()> {
    let (Frame::Image { plot_area, map }, Some(ip)) = (frame, out.image.as_ref()) else {
        return Ok(());
    };
    let Some(im) = info.images.iter().find(|i| i.index == ip.image) else {
        return Ok(());
    };
    let full = ip.full_res_region;
    // the finest level at which the view fits the count's budget
    let mut level = 0u32;
    let mut lsize = (im.size_x, im.size_y);
    for l in 0..im.pyramid_levels.max(1) {
        let Ok((w, h)) = openreadout_core::region::level_size(im, l) else {
            break;
        };
        level = l;
        lsize = (w, h);
        let (fx, fy) = (
            f64::from(im.size_x) / f64::from(w.max(1)),
            f64::from(im.size_y) / f64::from(h.max(1)),
        );
        let px = (f64::from(full.width) / fx) * (f64::from(full.height) / fy);
        if px <= MAX_COUNT_PIXELS as f64 {
            break;
        }
    }
    let (fx, fy) = (
        f64::from(im.size_x) / f64::from(lsize.0.max(1)),
        f64::from(im.size_y) / f64::from(lsize.1.max(1)),
    );
    let x0 = (f64::from(full.x) / fx).floor() as u32;
    let y0 = (f64::from(full.y) / fy).floor() as u32;
    let x1 = ((f64::from(full.x + full.width) / fx).ceil() as u32).min(lsize.0);
    let y1 = ((f64::from(full.y + full.height) / fy).ceil() as u32).min(lsize.1);
    let region = Region::new(
        x0,
        y0,
        x1.saturating_sub(x0).max(1),
        y1.saturating_sub(y0).max(1),
    );
    // the plane shown: its z and t, and its channel when it shows exactly one
    let mut select = vec![format!("z={}", ip.z.first().copied().unwrap_or(0))];
    select.push(format!("t={}", ip.t.first().copied().unwrap_or(0)));
    let explicit_c = req.select.iter().any(|s| s.contains('c'));
    if explicit_c && ip.c.len() == 1 && !ip.rgb {
        select.push(format!("c={}", ip.c[0]));
    }
    let mut o: ObjectsRequest = req.objects.clone().unwrap_or_default();
    o.outlines = true;
    let listed = o.max_objects.unwrap_or(LISTED);
    o.max_objects = Some(usize::MAX);
    let m = MeasureRequest {
        image: Some(ip.image),
        select,
        level,
        region: Some(region),
        objects: Some(o.clone()),
        ..MeasureRequest::default()
    };
    let res = measure(ds, info, &m)?;
    let Some(mut objs) = res.objects else {
        return Ok(());
    };
    if ip.projection.is_some() {
        out.notes.push(format!(
            "objects were counted in plane z={}, not in the projection",
            ip.z.first().copied().unwrap_or(0)
        ));
    }
    if level > 0 {
        out.notes.push(format!(
            "objects were counted at pyramid level {level} (coordinates in the JSON are its pixels); zoom in with region for a full-resolution count"
        ));
    }
    out.notes.extend(res.notes);

    // drawing: full-resolution px of the level's px, then figure px
    let to_fig = |x: f64, y: f64| -> (f32, f32) {
        let (px, py) = map.px_of(x * fx, y * fy);
        (
            plot_area.x as f32 + px as f32,
            plot_area.y as f32 + py as f32,
        )
    };
    let area = Rect::new(
        plot_area.x as f32,
        plot_area.y as f32,
        plot_area.width as f32,
        plot_area.height as f32,
    );
    let theme = crate::theme(req);
    let accent = theme.accent;
    let muted = Color::rgb(160, 160, 160);
    // a white rim under each outline keeps it visible on any image
    let width = (theme.hairline() * 1.6).max(1.2);
    let rim = Stroke::new(Color::rgb(255, 255, 255).alpha(0.55), width + 1.6).round();
    let screen_d = objs.expected_diameter_px * fx / map.per_pixel.0;
    fig.group(Layer::Annotation, "objects", |fig| {
        for ob in &objs.objects {
            let color = if ob.edge { muted } else { accent };
            let path = if objs.kind == Kind::Spots || ob.outline.len() < 3 {
                let (cx, cy) = to_fig(ob.x, ob.y);
                Path::circle(cx, cy, ((screen_d / 2.0) as f32).max(3.0))
            } else {
                Path::polygon(ob.outline.iter().map(|p| to_fig(p[0], p[1])))
            };
            if !path_near(&path, area) {
                continue;
            }
            fig.stroke(path.clone(), rim.clone());
            let mut st = Stroke::new(color, width).round();
            if ob.edge {
                st = st.dashed(width * 3.0, width * 2.0);
            }
            fig.stroke(path, st);
        }
        // numbers, when they fit
        let count = objs.objects.len();
        if count <= MAX_NUMBERED && screen_d >= 14.0 {
            let fs = (theme.small_size() * 0.9).max(9.0);
            let st = TextStyle::new(fs, Color::rgb(29, 31, 36))
                .face(Face::Mono)
                .align(Anchor::Middle, Baseline::Middle)
                .halo(Color::rgb(255, 255, 255).alpha(0.85), (fs * 0.22).max(2.0));
            for ob in &objs.objects {
                let (cx, cy) = to_fig(ob.x, ob.y);
                if area.contains(cx, cy) {
                    fig.text(cx, cy, &ob.id.to_string(), &st);
                }
            }
        }
    });
    // the count, top left of the picture (bare pictures, as the viewer shows, go without)
    let caption = caption(&objs);
    let fs = theme.font_size;
    let pill = Pill {
        text: TextStyle::new(fs, Color::rgb(29, 31, 36)).face(Face::SansMedium),
        fill: Color::rgb(247, 245, 240).alpha(0.92),
        stroke: None,
        pad: ((fs * 0.6).round(), (fs * 0.3).round()),
    };
    if req.axes {
        let (w, h) = pill.size(&caption);
        let r = Rect::new(area.x + 6.0, area.y + 6.0, w.min(area.w - 12.0), h);
        fig.group(Layer::Annotation, "object count", |fig| {
            pill.draw(fig, r, &caption);
        });
    }
    // the JSON: the summary and the first objects, in full-resolution pixels
    for ob in &mut objs.objects {
        ob.outline.clear();
    }
    objs.truncated = objs.objects.len() > listed;
    objs.objects.truncate(listed);
    out.objects = Some(objs);
    Ok(())
}

/// `23 nuclei · median 11.8 µm` (and how many touch the edge).
fn caption(o: &Objects) -> String {
    let what = match (o.kind, o.count) {
        (Kind::Nuclei, 1) => "nucleus",
        (Kind::Nuclei, _) => "nuclei",
        (Kind::Cells, 1) => "cell",
        (Kind::Cells, _) => "cells",
        (Kind::Spots, 1) => "spot",
        (Kind::Spots, _) => "spots",
    };
    let mut s = format!("{} {what}", o.count);
    if o.confidence == openreadout_core::measure::CountConfidence::Low {
        s.push_str(" · low confidence, check");
    }
    if o.kind != Kind::Spots
        && let Some(d) = &o.diameter_um
    {
        s.push_str(&format!(" · median {:.1} µm", d.median));
    }
    if o.touching_edge > 0 {
        s.push_str(&format!(" · {} at the edge", o.touching_edge));
    }
    s
}

/// True when the path's bounds overlap `area` (objects of a tile margin may lie outside).
fn path_near(p: &Path, area: Rect) -> bool {
    p.bounds().is_some_and(|(x0, y0, x1, y1)| {
        x1 >= area.x && y1 >= area.y && x0 <= area.right() && y0 <= area.bottom()
    })
}
