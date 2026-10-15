//! The caller's annotations: points, boxes, outlines, arrows, text, lines and bands drawn on a
//! preview, in the preview's own coordinates (full-resolution pixels on images, axis units on
//! plots, wells on plates). Labels are placed so they do not cover each other, and sit on a
//! paper pill (images) or carry a halo (plots), so they read on anything.

use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, Baseline, Color, Face, Figure, LabelPlacer, Path, Pill, Rect, ScaleKind, Stroke,
    TextStyle, Theme, arrow_parts,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Frame, FramePanel, KeySlot, PlotPanel, PxRect};

/// What an annotation draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationType {
    /// a dot at x, y (with its label beside it)
    Point,
    /// a rectangle: corner x, y and width, height
    Box,
    /// a closed outline through points
    Polygon,
    /// an open line through points
    Line,
    /// an arrow from x, y to x2, y2
    Arrow,
    /// the label alone, at x, y
    Text,
    /// a vertical line at x
    Vline,
    /// a horizontal line at y
    Hline,
    /// a vertical band from x to x2
    Vband,
    /// a horizontal band from y to y2
    Hband,
    /// a ring around a plate well (well)
    Well,
}

/// One annotation. Coordinates are those of the preview: full-resolution pixels on images (as
/// on the rulers), axis units on plots (as on the tick labels), and on plate curve grids the
/// time and value axes every card shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    /// What to draw.
    #[serde(rename = "type")]
    pub kind: AnnotationType,
    /// point, text: position; box: corner; arrow: tail; vline, vband: x (vband from x to x2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// point, text: position; box: corner; arrow: tail; hline, hband: y (hband from y to y2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// arrow: head; vband: other edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x2: Option<f64>,
    /// arrow: head; hband: other edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y2: Option<f64>,
    /// box: width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// box: height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// polygon, line: the vertices, `[[x, y], ...]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    /// Text drawn with the mark (the text itself for type text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `accent` (default, warm red), `ink`, `muted`, `white` or `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Trace previews with several panels: the channel whose panel it goes in (default: the
    /// first panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    /// Plates: the well (e.g. `B3`) for type well, or the one card a mark goes in on a curve
    /// grid (default: every card).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
}

impl Annotation {
    /// A point with a label.
    pub fn point(x: f64, y: f64, label: &str) -> Annotation {
        Annotation {
            kind: AnnotationType::Point,
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            width: None,
            height: None,
            points: Vec::new(),
            label: Some(label.to_string()),
            color: None,
            channel: None,
            well: None,
        }
    }

    /// An annotation of `kind` with nothing set.
    pub fn new(kind: AnnotationType) -> Annotation {
        Annotation {
            kind,
            x: None,
            y: None,
            x2: None,
            y2: None,
            width: None,
            height: None,
            points: Vec::new(),
            label: None,
            color: None,
            channel: None,
            well: None,
        }
    }
}

/// An annotation as drawn: the request, plus where it landed in PNG pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DrawnAnnotation {
    /// The annotation as given.
    #[serde(flatten)]
    pub annotation: Annotation,
    /// The colour drawn, `#RRGGBB`.
    pub drawn_color: String,
    /// The mark's key points in PNG pixels: the point, the box corners, the vertices, the
    /// arrow's tail and head, a line's or band's ends (one pair per card on a curve grid).
    pub px: Vec<[f64; 2]>,
    /// Where the label went, in PNG pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_px: Option<PxRect>,
    /// True when part of the mark lies outside what the picture shows (it is clipped).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
}

impl DrawnAnnotation {
    /// Halve the pixel positions along with the raster.
    pub(crate) fn halve(&mut self) {
        for p in &mut self.px {
            p[0] /= 2.0;
            p[1] /= 2.0;
        }
        if let Some(r) = &mut self.label_px {
            r.x /= 2.0;
            r.y /= 2.0;
            r.width /= 2.0;
            r.height /= 2.0;
        }
    }
}

fn scale_name(k: ScaleKind) -> String {
    match k {
        ScaleKind::Linear => "linear".into(),
        ScaleKind::Log => "log".into(),
        ScaleKind::Asinh(_) => "asinh".into(),
    }
}

/// The JSON description of a plot panel.
pub(crate) fn plot_panel(p: &FramePanel) -> PlotPanel {
    let pn = &p.panel;
    let r = pn.rect;
    PlotPanel {
        plot_area: r.into(),
        x: [pn.x.value(r.x), pn.x.value(r.right())],
        y: [pn.y.value(r.bottom()), pn.y.value(r.y)],
        x_scale: scale_name(pn.x.kind),
        y_scale: scale_name(pn.y.kind),
        channel: p.channel,
        well: p.well.map(well_name),
    }
}

/// `B3` for row 1, column 2 (zero-based); rows past Z continue AA, AB, …
pub(crate) fn well_name((r, c): (u32, u32)) -> String {
    format!("{}{}", row_name(r), c + 1)
}

/// `A` for row 0; rows past Z continue AA, AB, …
pub(crate) fn row_name(r: u32) -> String {
    if r < 26 {
        char::from(b'A' + r as u8).to_string()
    } else {
        format!(
            "{}{}",
            char::from(b'A' + ((r / 26 - 1) % 26) as u8),
            char::from(b'A' + (r % 26) as u8)
        )
    }
}

/// Parse `B3`, `b03`, `AA12` into zero-based (row, column).
pub(crate) fn parse_well_name(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let letters: String = s.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits = &s[letters.len()..];
    if letters.is_empty() || letters.len() > 2 || digits.is_empty() {
        return None;
    }
    let up = letters.to_ascii_uppercase();
    let b = up.as_bytes();
    let row = if b.len() == 1 {
        u32::from(b[0] - b'A')
    } else {
        (u32::from(b[0] - b'A') + 1) * 26 + u32::from(b[1] - b'A')
    };
    let col: u32 = digits.parse().ok()?;
    (col >= 1).then(|| (row, col - 1))
}

fn resolve_color(c: Option<&str>, theme: &Theme) -> Result<Color> {
    match c.map(str::trim) {
        None | Some("" | "accent") => Ok(theme.accent),
        Some("ink") => Ok(theme.ink),
        Some("muted") => Ok(theme.muted),
        Some("white") => Ok(Color::rgb(255, 255, 255)),
        Some(s) => Color::parse(s).ok_or_else(|| {
            Error::Usage(format!(
                "annotation color '{s}': use accent, ink, muted, white or #RRGGBB"
            ))
        }),
    }
}

fn need(v: Option<f64>, what: &str, kind: &str) -> Result<f64> {
    match v {
        Some(v) if v.is_finite() => Ok(v),
        _ => Err(Error::Usage(format!(
            "annotation type {kind} needs a finite {what}"
        ))),
    }
}

fn kind_name(k: AnnotationType) -> &'static str {
    match k {
        AnnotationType::Point => "point",
        AnnotationType::Box => "box",
        AnnotationType::Polygon => "polygon",
        AnnotationType::Line => "line",
        AnnotationType::Arrow => "arrow",
        AnnotationType::Text => "text",
        AnnotationType::Vline => "vline",
        AnnotationType::Hline => "hline",
        AnnotationType::Vband => "vband",
        AnnotationType::Hband => "hband",
        AnnotationType::Well => "well",
    }
}

/// Check that an annotation has what its type needs, before anything is drawn.
fn validate(a: &Annotation) -> Result<()> {
    let k = kind_name(a.kind);
    match a.kind {
        AnnotationType::Point => {
            need(a.x, "x", k)?;
            need(a.y, "y", k)?;
        }
        AnnotationType::Text => {
            need(a.x, "x", k)?;
            need(a.y, "y", k)?;
            if a.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                return Err(Error::Usage("annotation type text needs a label".into()));
            }
        }
        AnnotationType::Box => {
            need(a.x, "x", k)?;
            need(a.y, "y", k)?;
            need(a.width, "width", k)?;
            need(a.height, "height", k)?;
        }
        AnnotationType::Arrow => {
            need(a.x, "x", k)?;
            need(a.y, "y", k)?;
            need(a.x2, "x2", k)?;
            need(a.y2, "y2", k)?;
        }
        AnnotationType::Polygon | AnnotationType::Line => {
            let min = if a.kind == AnnotationType::Polygon {
                3
            } else {
                2
            };
            if a.points.len() < min || a.points.iter().flatten().any(|v| !v.is_finite()) {
                return Err(Error::Usage(format!(
                    "annotation type {k} needs at least {min} finite points [[x, y], ...]"
                )));
            }
        }
        AnnotationType::Vline => {
            need(a.x, "x", k)?;
        }
        AnnotationType::Hline => {
            need(a.y, "y", k)?;
        }
        AnnotationType::Vband => {
            need(a.x, "x", k)?;
            need(a.x2, "x2", k)?;
        }
        AnnotationType::Hband => {
            need(a.y, "y", k)?;
            need(a.y2, "y2", k)?;
        }
        AnnotationType::Well => {
            let w = a.well.as_deref().unwrap_or("");
            if parse_well_name(w).is_none() {
                return Err(Error::Usage(format!(
                    "annotation type well needs a well such as \"B3\" (got '{w}')"
                )));
            }
        }
    }
    if let Some(w) = a.well.as_deref()
        && parse_well_name(w).is_none()
    {
        return Err(Error::Usage(format!(
            "annotation well '{w}' is not a well name such as B3"
        )));
    }
    Ok(())
}

/// A coordinate system the marks are drawn in: data → figure pixels, the area it covers, and
/// whether labels sit on pills (images) or carry halos (plots).
struct Space<'a> {
    to_px: Box<dyn Fn(f64, f64) -> (f32, f32) + 'a>,
    area: Rect,
    on_image: bool,
    /// For lines and bands: the data value at the area's edges is not needed, the area is.
    x_only: Box<dyn Fn(f64) -> f32 + 'a>,
    y_only: Box<dyn Fn(f64) -> f32 + 'a>,
}

fn image_space(plot: openreadout_core::Region, map: crate::rulers::Mapping) -> Space<'static> {
    let (ox, oy) = (plot.x as f32, plot.y as f32);
    let m = map;
    let m2 = map;
    let m3 = map;
    Space {
        to_px: Box::new(move |x, y| {
            let (px, py) = m.px_of(x, y);
            (ox + px as f32, oy + py as f32)
        }),
        area: Rect::new(ox, oy, plot.width as f32, plot.height as f32),
        on_image: true,
        x_only: Box::new(move |x| ox + m2.px_of(x, 0.0).0 as f32),
        y_only: Box::new(move |y| oy + m3.px_of(0.0, y).1 as f32),
    }
}

fn panel_space(p: &FramePanel) -> Space<'_> {
    let pn = p.panel;
    // values a log scale cannot place go far off the panel (and are clipped)
    let far = |v: Option<f32>| v.unwrap_or(-1e5);
    Space {
        to_px: Box::new(move |x, y| (far(pn.x.px(x)), far(pn.y.px(y)))),
        area: pn.rect,
        on_image: false,
        x_only: Box::new(move |x| far(pn.x.px(x))),
        y_only: Box::new(move |y| far(pn.y.px(y))),
    }
}

/// How labels look.
struct Labels {
    pill: Pill,
    halo: TextStyle,
    placer: LabelPlacer,
}

impl Labels {
    /// Place and draw `text` near `(x, y)`; returns its box.
    fn put(
        &mut self,
        fig: &mut Figure,
        on_image: bool,
        x: f32,
        y: f32,
        text: &str,
        color: Color,
        gap: f32,
    ) -> Rect {
        if on_image {
            let (w, h) = self.pill.size(text);
            let r = self.placer.place(x, y, w, h, gap);
            // ink on a paper pill whatever the mark's colour: legible on any tissue
            self.pill.draw(fig, r, text);
            r
        } else {
            let st = self.halo.clone();
            let b = st.bounds(0.0, 0.0, text);
            let (w, h) = (b.w, b.h);
            let r = self.placer.place(x, y, w, h, gap);
            let mut s = st.align(Anchor::Start, Baseline::Top);
            s.color = color;
            fig.text(r.x, r.y + (h - b.h).max(0.0), text, &s);
            r
        }
    }
}

/// Draw `anns` into `fig` over `frame`. Errors (usage) when an annotation lacks what its type
/// needs or does not fit the preview kind.
pub(crate) fn draw(
    fig: &mut Figure,
    frame: &Frame,
    anns: &[Annotation],
    theme: &Theme,
    notes: &mut Vec<String>,
) -> Result<Vec<DrawnAnnotation>> {
    for a in anns {
        validate(a)?;
    }
    let fs = theme.font_size;
    let mut labels = Labels {
        pill: Pill {
            text: TextStyle::new(fs, Color::rgb(29, 31, 36)).face(Face::SansMedium),
            fill: Color::rgb(247, 245, 240).alpha(0.92),
            stroke: None,
            pad: ((fs * 0.6).round(), (fs * 0.3).round()),
        },
        halo: TextStyle::new(fs, theme.accent)
            .face(Face::SansMedium)
            .halo(theme.paper, (fs * 0.2).max(2.0)),
        placer: LabelPlacer::new(fig.rect().inset(2.0)),
    };
    let mut out = Vec::with_capacity(anns.len());
    let mut outside_any = false;
    let mut key_slot: Option<KeySlot> = None;
    for a in anns {
        let color = resolve_color(a.color.as_deref(), theme)?;
        let mut d = DrawnAnnotation {
            annotation: a.clone(),
            drawn_color: color.to_hex(),
            px: Vec::new(),
            label_px: None,
            outside: false,
        };
        match frame {
            Frame::Image { plot_area, map } => {
                if a.kind == AnnotationType::Well {
                    return Err(Error::Usage(
                        "annotation type well needs a plate preview; on images use point or box in full-resolution pixels".into(),
                    ));
                }
                let sp = image_space(*plot_area, *map);
                if out.is_empty() {
                    // labels stay on the data
                    labels.placer = LabelPlacer::new(sp.area.inset(2.0));
                }
                draw_one(fig, &sp, a, color, theme, &mut labels, &mut d);
            }
            Frame::Plots { panels, key } => {
                if a.kind == AnnotationType::Well {
                    if panels.iter().any(|p| p.well.is_some()) {
                        let w = a.well.as_deref().and_then(parse_well_name);
                        if let Some(p) = panels.iter().find(|p| p.well == w) {
                            let r = p.panel.rect.inset(-3.0);
                            ring(fig, r, color, theme, false);
                            d.px = vec![corner(r.x, r.y), corner(r.right(), r.bottom())];
                            if let Some(l) = a.label.as_deref() {
                                let b = labels.put(fig, false, r.right(), r.y, l, color, 2.0);
                                d.label_px = Some(b.into());
                            }
                        } else {
                            d.outside = true;
                        }
                        out.push(d);
                        continue;
                    }
                    return Err(Error::Usage(
                        "annotation type well needs a plate preview".into(),
                    ));
                }
                let targets: Vec<&FramePanel> = select_panels(panels, a)?;
                for (i, p) in targets.iter().enumerate() {
                    let sp = panel_space(p);
                    // a label once (on the first card of a curve grid)
                    let mut one = a.clone();
                    if i > 0 {
                        one.label = None;
                    }
                    let mut part = DrawnAnnotation {
                        annotation: one.clone(),
                        drawn_color: d.drawn_color.clone(),
                        px: Vec::new(),
                        label_px: None,
                        outside: false,
                    };
                    let compact = targets.len() > 1;
                    if compact {
                        draw_compact(fig, &sp, &one, color, theme, &mut part);
                    } else {
                        draw_one(fig, &sp, &one, color, theme, &mut labels, &mut part);
                    }
                    d.px.extend(part.px);
                    d.outside |= part.outside;
                    if part.label_px.is_some() {
                        d.label_px = part.label_px;
                    }
                }
                if targets.len() > 1
                    && let Some(l) = a.label.as_deref()
                {
                    // one label for the whole grid: an entry in the figure's key
                    if key_slot.is_none() {
                        key_slot = *key;
                    }
                    let entry = key_entry(fig, theme, &mut key_slot, a.kind, l, color);
                    let b = entry.unwrap_or_else(|| {
                        let r = targets[0].panel.rect;
                        labels.put(fig, false, r.x, r.y, l, color, 2.0)
                    });
                    d.label_px = Some(b.into());
                }
            }
            Frame::Wells(cells) => {
                let w = a.well.as_deref().and_then(parse_well_name);
                match (a.kind, w) {
                    (AnnotationType::Well | AnnotationType::Point, Some(w)) => {
                        if let Some((_, r)) = cells.iter().find(|(p, _)| *p == w) {
                            let r = r.inset(-2.5);
                            ring(fig, r, color, theme, true);
                            d.px = vec![corner(r.x, r.y), corner(r.right(), r.bottom())];
                            if let Some(l) = a.label.as_deref() {
                                let b = labels.put(fig, false, r.right(), r.y, l, color, 3.0);
                                d.label_px = Some(b.into());
                            }
                        } else {
                            d.outside = true;
                        }
                    }
                    _ => {
                        return Err(Error::Usage(
                            "plate heat maps take annotations of type well (with well, e.g. \"B3\")".into(),
                        ));
                    }
                }
            }
        }
        outside_any |= d.outside;
        out.push(d);
    }
    if outside_any {
        notes.push(
            "some annotations lie partly outside what the picture shows and are clipped (see annotations[].outside)".into(),
        );
    }
    Ok(out)
}

fn corner(x: f32, y: f32) -> [f64; 2] {
    let q = |v: f32| (f64::from(v) * 100.0).round() / 100.0;
    [q(x), q(y)]
}

fn select_panels<'a>(panels: &'a [FramePanel], a: &Annotation) -> Result<Vec<&'a FramePanel>> {
    if let Some(w) = a.well.as_deref() {
        let w = parse_well_name(w);
        let v: Vec<&FramePanel> = panels
            .iter()
            .filter(|p| p.well.is_some() && p.well == w)
            .collect();
        if v.is_empty() {
            return Err(Error::Usage(format!(
                "annotation well {} is not on this plate",
                a.well.as_deref().unwrap_or("")
            )));
        }
        return Ok(v);
    }
    if let Some(c) = a.channel {
        let v: Vec<&FramePanel> = panels.iter().filter(|p| p.channel == Some(c)).collect();
        if v.is_empty() {
            let have: Vec<String> = panels
                .iter()
                .filter_map(|p| p.channel.map(|c| c.to_string()))
                .collect();
            return Err(Error::Usage(format!(
                "annotation channel {c} is not drawn here (panels show channel {})",
                if have.is_empty() {
                    "-".into()
                } else {
                    have.join(", ")
                }
            )));
        }
        return Ok(v);
    }
    // curve grids: every card; trace panels: the first
    if panels.iter().any(|p| p.well.is_some()) {
        Ok(panels.iter().collect())
    } else {
        Ok(panels.iter().take(1).collect())
    }
}

/// Label an annotation drawn on every card of a grid as an entry of the figure's key: a swatch
/// of the mark, then the label. Returns its box, or `None` when the key has no room.
fn key_entry(
    fig: &mut Figure,
    theme: &Theme,
    slot: &mut Option<KeySlot>,
    kind: AnnotationType,
    label: &str,
    color: Color,
) -> Option<Rect> {
    let s = (*slot)?;
    let fs = theme.font_size;
    let st = TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Middle);
    let sw = fs * 1.1;
    let w = sw + fs * 0.4 + st.width(label);
    if s.x + w > s.max_x {
        return None;
    }
    let (x, y) = (s.x, s.y);
    match kind {
        AnnotationType::Vband | AnnotationType::Hband => {
            fig.fill(Path::rect(x, y - fs * 0.45, sw, fs * 0.9), color.alpha(0.2));
        }
        AnnotationType::Vline | AnnotationType::Hline => {
            fig.stroke(
                Path::segment(x, y, x + sw, y),
                Stroke::new(color, 1.5).dashed(3.0, 2.0),
            );
        }
        _ => {
            fig.fill(Path::circle(x + sw / 2.0, y, fs * 0.3), color);
        }
    }
    let b = fig.text(x + sw + fs * 0.4, y, label, &st);
    *slot = Some(KeySlot {
        x: b.right() + fs * 1.2,
        ..s
    });
    Some(Rect::new(x, y - fs * 0.6, b.right() - x, fs * 1.2))
}

/// A ring around a rectangle: a circle (`round`, for plate wells) or a rounded rectangle.
fn ring(fig: &mut Figure, r: Rect, color: Color, theme: &Theme, round: bool) {
    let w = (theme.line_width * 1.3).max(2.0);
    let path = if round {
        let (cx, cy) = r.center();
        Path::circle(cx, cy, r.w.max(r.h) / 2.0)
    } else {
        Path::round_rect(r.x, r.y, r.w, r.h, (r.h.min(r.w) * 0.12).min(8.0))
    };
    fig.stroke(path, Stroke::new(color, w));
}

/// Draw on a small card of a curve grid: lines, bands, points only, no labels.
fn draw_compact(
    fig: &mut Figure,
    sp: &Space<'_>,
    a: &Annotation,
    color: Color,
    theme: &Theme,
    d: &mut DrawnAnnotation,
) {
    let r = sp.area;
    let clip = Some(r);
    match a.kind {
        AnnotationType::Vband => {
            let (x0, x1) = (
                (sp.x_only)(a.x.unwrap_or(0.0)),
                (sp.x_only)(a.x2.unwrap_or(0.0)),
            );
            let (x0, x1) = (x0.min(x1).max(r.x), x0.max(x1).min(r.right()));
            if x1 > x0 {
                fig.shape(
                    Path::rect(x0, r.y, x1 - x0, r.h),
                    Some(color.alpha(0.16)),
                    None,
                    clip,
                );
            }
            d.px.push(corner(x0, r.y));
            d.px.push(corner(x1, r.bottom()));
        }
        AnnotationType::Hband => {
            let (y0, y1) = (
                (sp.y_only)(a.y.unwrap_or(0.0)),
                (sp.y_only)(a.y2.unwrap_or(0.0)),
            );
            let (y0, y1) = (y0.min(y1).max(r.y), y0.max(y1).min(r.bottom()));
            if y1 > y0 {
                fig.shape(
                    Path::rect(r.x, y0, r.w, y1 - y0),
                    Some(color.alpha(0.16)),
                    None,
                    clip,
                );
            }
            d.px.push(corner(r.x, y0));
            d.px.push(corner(r.right(), y1));
        }
        AnnotationType::Vline => {
            let x = (sp.x_only)(a.x.unwrap_or(0.0));
            fig.shape(
                Path::segment(x, r.y, x, r.bottom()),
                None,
                Some(Stroke::new(color, 1.0).dashed(3.0, 2.0)),
                clip,
            );
            d.outside |= !(r.x..=r.right()).contains(&x);
            d.px.push(corner(x, r.y));
        }
        AnnotationType::Hline => {
            let y = (sp.y_only)(a.y.unwrap_or(0.0));
            fig.shape(
                Path::segment(r.x, y, r.right(), y),
                None,
                Some(Stroke::new(color, 1.0).dashed(3.0, 2.0)),
                clip,
            );
            d.outside |= !(r.y..=r.bottom()).contains(&y);
            d.px.push(corner(r.x, y));
        }
        _ => {
            // other marks keep their shape, small
            let mut quiet = Labels {
                pill: Pill {
                    text: TextStyle::new(1.0, color),
                    fill: color,
                    stroke: None,
                    pad: (0.0, 0.0),
                },
                halo: TextStyle::new(theme.small_size(), color),
                placer: LabelPlacer::new(r),
            };
            let mut one = a.clone();
            one.label = None;
            draw_one(fig, sp, &one, color, theme, &mut quiet, d);
        }
    }
}

/// Draw one annotation in `sp`.
fn draw_one(
    fig: &mut Figure,
    sp: &Space<'_>,
    a: &Annotation,
    color: Color,
    theme: &Theme,
    labels: &mut Labels,
    d: &mut DrawnAnnotation,
) {
    let r = sp.area;
    let lw = (theme.line_width * 1.3).max(2.0);
    let halo = if sp.on_image {
        Color::rgb(255, 255, 255).alpha(0.7)
    } else {
        theme.paper.alpha(0.85)
    };
    let stroke_with_halo = |fig: &mut Figure, p: Path, closed_fill: Option<Color>| {
        fig.shape(
            p.clone(),
            None,
            Some(Stroke::new(halo, lw + 2.5).round()),
            Some(r),
        );
        fig.shape(
            p,
            closed_fill,
            Some(Stroke::new(color, lw).round()),
            Some(r),
        );
    };
    let fs = theme.font_size;
    let inside = |x: f32, y: f32| r.inset(-0.5).contains(x, y);
    let label = a.label.as_deref().filter(|l| !l.trim().is_empty());
    match a.kind {
        AnnotationType::Point => {
            let (x, y) = (sp.to_px)(a.x.unwrap_or(0.0), a.y.unwrap_or(0.0));
            d.outside = !inside(x, y);
            d.px.push(corner(x, y));
            let rad = (fs * 0.42).max(4.0);
            if !d.outside {
                if sp.on_image {
                    // a ring with a white rim: visible on dark and bright tissue
                    fig.stroke(
                        Path::circle(x, y, rad),
                        Stroke::new(Color::rgb(255, 255, 255).alpha(0.85), lw + 2.5),
                    );
                    fig.stroke(Path::circle(x, y, rad), Stroke::new(color, lw));
                    fig.fill(Path::circle(x, y, (lw * 0.8).max(1.5)), color);
                } else {
                    fig.shape(
                        Path::circle(x, y, rad * 0.75),
                        Some(color),
                        Some(Stroke::new(theme.paper, (lw * 0.6).max(1.2))),
                        None,
                    );
                }
                labels
                    .placer
                    .reserve(Rect::new(x - rad, y - rad, rad * 2.0, rad * 2.0));
            }
            if let Some(l) = label {
                let b = labels.put(fig, sp.on_image, x, y, l, color, rad + 3.0);
                d.label_px = Some(b.into());
            }
        }
        AnnotationType::Text => {
            let (x, y) = (sp.to_px)(a.x.unwrap_or(0.0), a.y.unwrap_or(0.0));
            d.outside = !inside(x, y);
            d.px.push(corner(x, y));
            if let Some(l) = label {
                let b = if sp.on_image {
                    let (w, h) = labels.pill.size(l);
                    let rr = Rect::new(x - w / 2.0, y - h / 2.0, w, h);
                    labels.placer.reserve(rr);
                    labels.pill.draw(fig, rr, l);
                    rr
                } else {
                    let st = labels.halo.clone().align(Anchor::Middle, Baseline::Middle);
                    let mut st = st;
                    st.color = color;
                    let b = fig.text(x, y, l, &st);
                    labels.placer.reserve(b);
                    b
                };
                d.label_px = Some(b.into());
            }
        }
        AnnotationType::Box => {
            let (x, y) = (a.x.unwrap_or(0.0), a.y.unwrap_or(0.0));
            let (x2, y2) = (x + a.width.unwrap_or(0.0), y + a.height.unwrap_or(0.0));
            let (p0x, p0y) = (sp.to_px)(x, y);
            let (p1x, p1y) = (sp.to_px)(x2, y2);
            let b = Rect::from_corners(p0x, p0y, p1x, p1y);
            d.outside = !r.inset(-0.5).contains_rect(&b);
            d.px.push(corner(b.x, b.y));
            d.px.push(corner(b.right(), b.bottom()));
            stroke_with_halo(fig, Path::rect(b.x, b.y, b.w.max(1.0), b.h.max(1.0)), None);
            labels
                .placer
                .reserve(Rect::new(b.x - 2.0, b.y - 2.0, b.w + 4.0, 4.0));
            if let Some(l) = label {
                // above the top-left corner, else wherever it fits
                let bb = labels.put(fig, sp.on_image, b.x, b.y, l, color, 4.0);
                d.label_px = Some(bb.into());
            }
        }
        AnnotationType::Polygon | AnnotationType::Line => {
            let pts: Vec<(f32, f32)> = a.points.iter().map(|p| (sp.to_px)(p[0], p[1])).collect();
            d.outside = pts.iter().any(|&(x, y)| !inside(x, y));
            d.px = pts.iter().map(|&(x, y)| corner(x, y)).collect();
            let path = if a.kind == AnnotationType::Polygon {
                Path::polygon(pts.iter().copied())
            } else {
                Path::polyline(pts.iter().copied())
            };
            let fill = (a.kind == AnnotationType::Polygon).then(|| color.alpha(0.12));
            stroke_with_halo(fig, path, fill);
            if let Some(l) = label {
                // next to the topmost vertex
                let top = pts
                    .iter()
                    .copied()
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((r.x, r.y));
                let bb = labels.put(fig, sp.on_image, top.0, top.1, l, color, 4.0);
                d.label_px = Some(bb.into());
            }
        }
        AnnotationType::Arrow => {
            let (x0, y0) = (sp.to_px)(a.x.unwrap_or(0.0), a.y.unwrap_or(0.0));
            let (x1, y1) = (sp.to_px)(a.x2.unwrap_or(0.0), a.y2.unwrap_or(0.0));
            d.outside = !inside(x0, y0) || !inside(x1, y1);
            d.px = vec![corner(x0, y0), corner(x1, y1)];
            if let Some((shaft, head)) = arrow_parts(x0, y0, x1, y1, lw) {
                let rim = Stroke::new(halo, 2.5).round();
                fig.shape(
                    shaft.clone(),
                    None,
                    Some(Stroke::new(halo, lw + 2.5).round()),
                    Some(r),
                );
                fig.shape(head.clone(), None, Some(rim), Some(r));
                fig.shape(shaft, None, Some(Stroke::new(color, lw).round()), Some(r));
                fig.shape(head, Some(color), None, Some(r));
            }
            if let Some(l) = label {
                // at the tail, on the side away from the head
                let (dx, dy) = (x0 - x1, y0 - y1);
                let n = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (ax, ay) = (x0 + dx / n * 4.0, y0 + dy / n * 4.0);
                let bb = labels.put(fig, sp.on_image, ax, ay, l, color, 3.0);
                d.label_px = Some(bb.into());
            }
        }
        AnnotationType::Vline | AnnotationType::Hline => {
            let vertical = a.kind == AnnotationType::Vline;
            let (p0, p1) = if vertical {
                let x = (sp.x_only)(a.x.unwrap_or(0.0));
                ((x, r.y), (x, r.bottom()))
            } else {
                let y = (sp.y_only)(a.y.unwrap_or(0.0));
                ((r.x, y), (r.right(), y))
            };
            d.outside = !inside(p0.0, p0.1);
            d.px = vec![corner(p0.0, p0.1), corner(p1.0, p1.1)];
            let path = Path::segment(p0.0, p0.1, p1.0, p1.1);
            fig.shape(
                path.clone(),
                None,
                Some(Stroke::new(halo, lw + 1.5)),
                Some(r),
            );
            fig.shape(
                path,
                None,
                Some(Stroke::new(color, lw * 0.75).dashed(lw * 3.0, lw * 2.0)),
                Some(r),
            );
            if let Some(l) = label {
                let (ax, ay) = if vertical {
                    (p0.0, r.y + fs * 0.2)
                } else {
                    (r.right() - fs * 0.2, p0.1)
                };
                let bb = labels.put(fig, sp.on_image, ax, ay, l, color, 3.0);
                d.label_px = Some(bb.into());
            }
        }
        AnnotationType::Vband | AnnotationType::Hband => {
            let vertical = a.kind == AnnotationType::Vband;
            let b = if vertical {
                let (x0, x1) = (
                    (sp.x_only)(a.x.unwrap_or(0.0)),
                    (sp.x_only)(a.x2.unwrap_or(0.0)),
                );
                Rect::from_corners(x0, r.y, x1, r.bottom())
            } else {
                let (y0, y1) = (
                    (sp.y_only)(a.y.unwrap_or(0.0)),
                    (sp.y_only)(a.y2.unwrap_or(0.0)),
                );
                Rect::from_corners(r.x, y0, r.right(), y1)
            };
            d.outside = !r.inset(-0.5).contains_rect(&b);
            d.px = vec![corner(b.x, b.y), corner(b.right(), b.bottom())];
            let fill = if sp.on_image {
                color.alpha(0.2)
            } else {
                color.alpha(0.13)
            };
            fig.shape(
                Path::rect(b.x, b.y, b.w.max(0.5), b.h.max(0.5)),
                Some(fill),
                None,
                Some(r),
            );
            // thin edges keep a narrow band visible
            let edge = Stroke::new(color.alpha(0.55), 1.0);
            if vertical {
                fig.shape(
                    Path::segment(b.x, b.y, b.x, b.bottom()),
                    None,
                    Some(edge.clone()),
                    Some(r),
                );
                fig.shape(
                    Path::segment(b.right(), b.y, b.right(), b.bottom()),
                    None,
                    Some(edge),
                    Some(r),
                );
            } else {
                fig.shape(
                    Path::segment(b.x, b.y, b.right(), b.y),
                    None,
                    Some(edge.clone()),
                    Some(r),
                );
                fig.shape(
                    Path::segment(b.x, b.bottom(), b.right(), b.bottom()),
                    None,
                    Some(edge),
                    Some(r),
                );
            }
            if let Some(l) = label {
                let (cx, _) = b.center();
                let (ax, ay) = if vertical {
                    (cx, r.y + fs * 0.3)
                } else {
                    (r.right() - fs * 0.3, b.y)
                };
                let st = TextStyle::new(fs, color)
                    .face(Face::SansMedium)
                    .halo(theme.paper, (fs * 0.2).max(2.0));
                let bb = if vertical {
                    let s = st.align(Anchor::Middle, Baseline::Top);
                    let tb = s.bounds(ax, ay, l);
                    let tb = Rect::new(
                        tb.x.clamp(r.x, (r.right() - tb.w).max(r.x)),
                        tb.y,
                        tb.w,
                        tb.h,
                    );
                    fig.text(tb.x + tb.w / 2.0, ay, l, &s);
                    tb
                } else {
                    labels.put(fig, sp.on_image, ax, ay, l, color, 3.0)
                };
                labels.placer.reserve(bb);
                d.label_px = Some(bb.into());
            }
        }
        AnnotationType::Well => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wells_round_trip() {
        assert_eq!(well_name((1, 2)), "B3");
        assert_eq!(parse_well_name("b03"), Some((1, 2)));
        assert_eq!(parse_well_name("AF48"), Some((31, 47)));
        assert_eq!(well_name((31, 47)), "AF48");
        assert_eq!(parse_well_name("3B"), None);
        assert_eq!(parse_well_name("B0"), None);
    }

    #[test]
    fn annotations_parse_and_validate() {
        let a: Annotation =
            serde_json::from_str(r#"{"type":"point","x":10,"y":20,"label":"cell"}"#).unwrap();
        assert!(validate(&a).is_ok());
        let bad: Annotation = serde_json::from_str(r#"{"type":"box","x":1,"y":2}"#).unwrap();
        assert!(validate(&bad).unwrap_err().to_string().contains("width"));
        assert!(
            serde_json::from_str::<Annotation>(r#"{"type":"point","x":1,"y":2,"caption":"x"}"#)
                .is_err()
        );
        let t: Annotation = serde_json::from_str(r#"{"type":"text","x":1,"y":2}"#).unwrap();
        assert!(validate(&t).is_err());
        let w: Annotation = serde_json::from_str(r#"{"type":"well","well":"H12"}"#).unwrap();
        assert!(validate(&w).is_ok());
    }
}
