//! `marks: lanes` on image previews: find the lanes and bands of a gel or blot in what the
//! picture shows (`openreadout_core::measure`, the same analysis as `stats` lanes) and draw
//! each lane's box and number, its profile, and its bands with their share of the strongest.

use openreadout_core::measure::{LanesRequest, MeasureRequest, measure};
use openreadout_core::{Dataset, FileInfo, Region, Result};
use openreadout_figure::{
    Anchor, Baseline, Color, Face, Figure, Path, Pill, Rect, Stroke, TextStyle,
};

use crate::{Frame, PreviewOutput, PreviewRequest};

/// Band labels are drawn when the lanes are at least this wide on screen.
const MIN_LABEL_LANE_PX: f64 = 26.0;

/// Find lanes in the area the preview shows and draw them over the image.
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
    let region = (!full.is_full(im.size_x, im.size_y)).then_some(full);
    let mut select = vec![format!("z={}", ip.z.first().copied().unwrap_or(0))];
    select.push(format!("t={}", ip.t.first().copied().unwrap_or(0)));
    if let Some(c) = ip.c.first() {
        select.push(format!("c={c}"));
    }
    let mut lr: LanesRequest = req.lanes.clone().unwrap_or_default();
    lr.profiles = true;
    let m = MeasureRequest {
        image: Some(ip.image),
        select,
        level: 0,
        region: region.map(|r| Region::new(r.x, r.y, r.width, r.height)),
        lanes: Some(lr),
        ..MeasureRequest::default()
    };
    let res = match measure(ds, info, &m) {
        Ok(r) => r,
        Err(e) => {
            out.notes.push(format!("lanes: {e}"));
            return Ok(());
        }
    };
    let Some(mut lanes) = res.lanes else {
        return Ok(());
    };
    out.notes.extend(res.notes);

    let to_fig = |x: f64, y: f64| -> (f32, f32) {
        let (px, py) = map.px_of(x, y);
        (
            plot_area.x as f32 + px as f32,
            plot_area.y as f32 + py as f32,
        )
    };
    let theme = crate::theme(req);
    let accent = theme.accent;
    let ink = Color::rgb(29, 31, 36);
    let paper = Color::rgb(255, 255, 255);
    let hair = (theme.hairline() * 1.4).max(1.0);
    let rim = Stroke::new(paper.alpha(0.6), hair + 1.6).round();
    let fs = (theme.small_size() * 0.95).max(9.0);
    let num_style = TextStyle::new(fs, ink)
        .face(Face::SansMedium)
        .align(Anchor::Middle, Baseline::Bottom)
        .halo(paper.alpha(0.9), (fs * 0.25).max(2.0));
    let band_style = TextStyle::new((fs * 0.85).max(8.0), ink)
        .face(Face::Mono)
        .align(Anchor::Start, Baseline::Middle)
        .halo(paper.alpha(0.9), (fs * 0.22).max(2.0));
    let step = lanes.profile_step_px.unwrap_or(1.0);
    let top_y = lanes.y_range[0];
    for lane in &lanes.lanes {
        let (x0, y0) = to_fig(lane.x_range[0], lanes.y_range[0]);
        let (x1, y1) = to_fig(lane.x_range[1], lanes.y_range[1]);
        let box_path = Path::rect(x0, y0, x1 - x0, y1 - y0);
        fig.stroke(box_path.clone(), rim.clone());
        let mut st = Stroke::new(accent.alpha(0.75), hair);
        if lane.empty {
            st = st.dashed(hair * 3.0, hair * 2.5);
        }
        fig.stroke(box_path, st);
        // the profile, rising to the right inside the lane
        let lo = lane
            .background
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let hi = lane
            .profile
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        if hi > lo && lane.profile.len() > 1 {
            let wpx = f64::from(x1 - x0) * 0.9;
            let pts: Vec<(f32, f32)> = lane
                .profile
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    let (_, fy) = to_fig(lane.x_range[0], top_y + k as f64 * step);
                    let fx = x0 as f64 + 0.05 * f64::from(x1 - x0) + (v - lo) / (hi - lo) * wpx;
                    (fx as f32, fy)
                })
                .collect();
            let path = Path::polyline(pts);
            fig.stroke(
                path.clone(),
                Stroke::new(paper.alpha(0.7), hair + 1.4).round(),
            );
            fig.stroke(path, Stroke::new(ink.alpha(0.85), hair * 0.8).round());
        }
        // bands: a tick across the lane, and the share of the strongest band
        let lane_screen = f64::from(x1 - x0);
        for b in &lane.bands {
            let (_, by) = to_fig(lane.x, b.y);
            let tick = Path::segment(x0, by, x1, by);
            fig.stroke(tick.clone(), rim.clone());
            let color = if b.saturated_px > 0 {
                Color::rgb(214, 40, 40)
            } else {
                accent
            };
            fig.stroke(tick, Stroke::new(color, hair * 1.6));
            if lane_screen >= MIN_LABEL_LANE_PX {
                let mut label = format!("{:.0}", b.relative * 100.0);
                if let Some(s) = b.size {
                    label = fmt_size(s);
                }
                if b.saturated_px > 0 {
                    label.push('!');
                }
                fig.text(
                    x1 - (lane_screen as f32) * 0.45,
                    by - fs * 0.6,
                    &label,
                    &band_style,
                );
            }
        }
        // lane number above the box
        fig.text(
            (x0 + x1) / 2.0,
            y0 + fs * 1.3,
            &lane.number.to_string(),
            &num_style,
        );
    }
    // the summary, bottom left of the picture
    if req.axes {
        let mut caption = format!("{} lanes · {} bands", lanes.count, lanes.band_count);
        let sat = lanes
            .lanes
            .iter()
            .flat_map(|l| &l.bands)
            .filter(|b| b.saturated_px > 0)
            .count();
        if sat > 0 {
            caption.push_str(&format!(" · {sat} saturated (!)"));
        }
        caption.push_str(
            if lanes
                .lanes
                .iter()
                .any(|l| l.bands.iter().any(|b| b.size.is_some()))
            {
                " · labels: size"
            } else {
                " · labels: % of strongest"
            },
        );
        let fsz = theme.font_size;
        let pill = Pill {
            text: TextStyle::new(fsz, ink).face(Face::SansMedium),
            fill: Color::rgb(247, 245, 240).alpha(0.92),
            stroke: None,
            pad: ((fsz * 0.6).round(), (fsz * 0.3).round()),
        };
        let area = Rect::new(
            plot_area.x as f32,
            plot_area.y as f32,
            plot_area.width as f32,
            plot_area.height as f32,
        );
        let (w, h) = pill.size(&caption);
        let r = Rect::new(
            area.x + 6.0,
            area.bottom() - h - 6.0,
            w.min(area.w - 12.0),
            h,
        );
        pill.draw(fig, r, &caption);
    }
    // the JSON: lanes and bands without the profiles
    for l in &mut lanes.lanes {
        l.profile.clear();
        l.background.clear();
    }
    lanes.profile_step_px = None;
    out.lanes = Some(lanes);
    Ok(())
}

fn fmt_size(s: f64) -> String {
    if s >= 100.0 {
        format!("{s:.0}")
    } else if s >= 10.0 {
        format!("{s:.1}")
    } else {
        format!("{s:.2}")
    }
}
