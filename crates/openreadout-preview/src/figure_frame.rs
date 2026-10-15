//! The frame of an image preview meant for people (`axes: "scale-bar"`): the picture on paper
//! inside a hairline, a scale bar under its right edge, and, when given, a title above and a
//! caption below at the left. No rulers: the agent reads coordinates off the ruler frame, a
//! reader of a saved figure needs the scale and what is shown.
//!
//! ```text
//!   Title, wrapped to the picture's width
//!   ┌──────────────────────────────┐
//!   │            image             │
//!   └──────────────────────────────┘
//!   Caption, wrapped beside        200 µm ━━━━
//!   the scale bar
//! ```

use std::sync::Arc;

use openreadout_core::region::Region;
use openreadout_figure::{
    Anchor, Baseline, Face, Figure, Path, Raster, Rect, Stroke, TextStyle, Theme, text_width,
};

use crate::rulers::{Decorated, Mapping, fmt_len, label_size, scale_bar};

/// Most lines of a title.
const MAX_TITLE_LINES: usize = 2;
/// Most lines of a caption.
const MAX_CAPTION_LINES: usize = 4;

/// Margins of the frame, in whole pixels, and the text it holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FigureLayout {
    /// Base type size (scale bar label and caption).
    pub fs: f32,
    /// Title type size.
    pub title_fs: f32,
    /// Paper around everything.
    pub pad: u32,
    /// Space above the data (padding, title).
    pub top: u32,
    /// Space below the data (scale bar row, caption, padding).
    pub bottom: u32,
    /// The narrowest the text block may be: a portrait picture gets a wider frame so the
    /// title and caption are not squeezed into a column (0 without text).
    pub min_width: u32,
    /// Whether a scale bar is drawn (the pixel size is known).
    pub bar: bool,
    pub title: Option<String>,
    pub caption: Option<String>,
}

/// Lines of `text` at most `max_w` pixels wide, broken at spaces; the last kept line ends in
/// `…` when lines are dropped.
pub(crate) fn wrap(text: &str, face: Face, size: f32, max_w: f32, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let trial = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if !cur.is_empty() && text_width(&trial, face, size) > max_w {
            lines.push(std::mem::replace(&mut cur, word.to_string()));
        } else {
            cur = trial;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() && text_width(&format!("{last} …"), face, size) > max_w {
                last.pop();
            }
            last.push_str(" …");
        }
    }
    lines
}

fn line_h(fs: f32) -> f32 {
    (fs * 1.4).round()
}

/// Width taken by the scale bar and its label at the right of the bottom row, for a bar about
/// a fifth of `data_w`.
fn bar_block(fs: f32, data_w: f32) -> f32 {
    data_w / 5.0 + text_width("500 µm", Face::Sans, fs) + fs * 1.5
}

impl FigureLayout {
    /// Margins for a preview of longest side `max_size`, with a scale bar when `bar`.
    pub fn new(max_size: u32, bar: bool, title: Option<&str>, caption: Option<&str>) -> Self {
        let fs = label_size(max_size);
        let title_fs = (fs * 1.35).round();
        let pad = (fs * 1.3).round() as u32;
        let has_text = title.is_some() || caption.is_some();
        let min_width = if has_text {
            (max_size as f32 * 0.62).round() as u32
        } else {
            0
        };
        let w = min_width as f32;
        let title_lines = title.map_or(0, |t| {
            wrap(t, Face::SansMedium, title_fs, w, MAX_TITLE_LINES).len()
        });
        // beside the scale bar, or under it at full width: room for the taller of the two
        let caption_rows = caption.map_or(0, |c| {
            let full = wrap(c, Face::Sans, fs, w, MAX_CAPTION_LINES).len();
            if bar {
                let beside_w = w - bar_block(fs, w);
                wrap(c, Face::Sans, fs, beside_w, MAX_CAPTION_LINES)
                    .len()
                    .max(full + 1)
            } else {
                full
            }
        });
        let gap = fs * 0.8;
        let top = pad as f32
            + if title_lines > 0 {
                title_lines as f32 * line_h(title_fs) + gap
            } else {
                0.0
            };
        let rows = caption_rows.max(usize::from(bar));
        let bottom = pad as f32
            + if rows > 0 {
                gap + rows as f32 * line_h(fs)
            } else {
                0.0
            };
        FigureLayout {
            fs,
            title_fs,
            pad,
            top: top.ceil() as u32,
            bottom: bottom.ceil() as u32,
            min_width,
            bar,
            title: title.map(str::to_string),
            caption: caption.map(str::to_string),
        }
    }

    /// Horizontal margins.
    pub fn h(&self) -> u32 {
        2 * self.pad
    }

    /// Vertical margins.
    pub fn v(&self) -> u32 {
        self.top + self.bottom
    }

    /// The longer side of the data so that the picture, frame included, fits `max_size` both
    /// ways for a `w`×`h` plane.
    pub fn fit(&self, max_size: u32, w: u32, h: u32) -> u32 {
        let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let room_w = f64::from(max_size.saturating_sub(self.h()));
        let room_h = f64::from(max_size.saturating_sub(self.v()));
        let long = if w >= h {
            room_w.min(room_h * w / h)
        } else {
            room_h.min(room_w * h / w)
        };
        (long.floor() as u32).max(1)
    }
}

/// Frame `data` as a figure for people (see the module notes).
pub(crate) fn decorate(
    data: Raster,
    layout: &FigureLayout,
    map: Mapping,
    um_per_source_px: Option<f64>,
    theme: &Theme,
) -> Decorated {
    let fs = layout.fs;
    let (dw, dh) = (data.width, data.height);
    let inner_w = dw.max(layout.min_width);
    let pad = layout.pad as f32;
    let gap = fs * 0.8;
    let bar = um_per_source_px.and_then(|um| scale_bar(um * map.per_pixel.0, dw));
    let bar_label = bar
        .as_ref()
        .map(|b| format!("{} {}", fmt_len(b.length), b.unit));
    let title_lines = layout.title.as_deref().map_or_else(Vec::new, |t| {
        wrap(
            t,
            Face::SansMedium,
            layout.title_fs,
            inner_w as f32,
            MAX_TITLE_LINES,
        )
    });
    // the data, centred when the text block is wider
    let l = (layout.pad + (inner_w - dw) / 2) as f32;
    let (w, h) = (dw as f32, dh as f32);
    let text_l = pad;
    // A caption sits beside the scale bar when there is room (ending before the bar's label),
    // else on the rows under it at full width.
    let (caption_lines, caption_row) = layout.caption.as_deref().map_or((Vec::new(), 0), |c| {
        let full = wrap(c, Face::Sans, fs, inner_w as f32, MAX_CAPTION_LINES);
        let (Some(b), Some(t)) = (&bar, &bar_label) else {
            return (full, 0);
        };
        let label_l = l + w - b.pixels as f32 - fs * 0.6 - text_width(t, Face::Sans, fs);
        let beside_w = label_l - fs * 1.2 - text_l;
        match (beside_w >= fs * 10.0)
            .then(|| wrap(c, Face::Sans, fs, beside_w, MAX_CAPTION_LINES))
            .filter(|lines| lines.len() <= full.len() + 1)
        {
            Some(lines) => (lines, 0),
            None => (full, 1),
        }
    });
    // the reserved rows are an upper bound; use what the text needs
    let top = pad
        + if title_lines.is_empty() {
            0.0
        } else {
            title_lines.len() as f32 * line_h(layout.title_fs) + gap
        };
    let rows = (caption_row + caption_lines.len()).max(usize::from(bar.is_some()));
    let bottom = pad
        + if rows > 0 {
            gap + rows as f32 * line_h(fs)
        } else {
            0.0
        };
    let top = top.ceil();
    let cw = inner_w + layout.h();
    let ch = dh + top as u32 + bottom.ceil() as u32;
    let mut fig = Figure::new(cw as f32, ch as f32, Some(theme.paper));
    fig.image(Rect::new(l, top, w, h), Arc::new(data), false);
    fig.stroke(
        Path::rect(l - 0.5, top - 0.5, w + 1.0, h + 1.0),
        Stroke::new(theme.rule, 1.0),
    );
    // title, flush with the text block's left edge
    let tst = TextStyle::new(layout.title_fs, theme.ink)
        .face(Face::SansMedium)
        .align(Anchor::Start, Baseline::Alphabetic);
    for (i, line) in title_lines.iter().enumerate() {
        let y = pad + (i as f32 + 0.8) * line_h(layout.title_fs);
        fig.text(text_l, y, line, &tst);
    }
    // bottom: caption at the left, scale bar at the right of the first row
    let row0 = top + h + gap + line_h(fs) / 2.0;
    if let (Some(b), Some(text)) = (&bar, &bar_label) {
        let bar_px = b.pixels as f32;
        let bx = l + w - bar_px;
        let thick = (fs * 0.3).round().max(2.0);
        fig.fill_rect(Rect::new(bx, row0 - thick / 2.0, bar_px, thick), theme.ink);
        let st = TextStyle::new(fs, theme.ink).align(Anchor::End, Baseline::Middle);
        fig.text(bx - fs * 0.6, row0, text, &st);
    }
    let cst = TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Middle);
    for (i, line) in caption_lines.iter().enumerate() {
        let row = (caption_row + i) as f32;
        fig.text(text_l, row0 + row * line_h(fs), line, &cst);
    }
    Decorated {
        figure: fig,
        plot_area: Region::new(l as u32, top as u32, dw, dh),
        scale_bar: bar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_keeps_lines_inside_and_marks_cuts() {
        let lines = wrap(
            "Sagittal section of a young mouse cerebellum, Masson trichrome, five layers labelled",
            Face::Sans,
            12.0,
            150.0,
            2,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|l| text_width(l, Face::Sans, 12.0) <= 150.0 + 0.5)
        );
    }

    #[test]
    fn the_frame_fits_max_size_both_ways() {
        let l = FigureLayout::new(800, true, Some("A title"), Some("A caption"));
        for (w, h) in [(4000, 1000), (1000, 4000), (1000, 1000)] {
            let long = l.fit(800, w, h);
            let (dw, dh) = if w >= h {
                (long, long * h / w)
            } else {
                (long * w / h, long)
            };
            assert!(dw.max(l.min_width) + l.h() <= 800, "{w}x{h}: {dw}");
            assert!(dh + l.v() <= 800, "{w}x{h}: {dh}");
        }
    }

    #[test]
    fn decoration_places_the_data_and_the_bar() {
        let theme = Theme::light().for_size(600.0);
        let l = FigureLayout::new(600, true, Some("Cerebellum"), None);
        let map = Mapping {
            origin: (0.0, 0.0),
            per_pixel: (2.0, 2.0),
        };
        let d = decorate(Raster::new(500, 300, [0, 0, 0]), &l, map, Some(0.5), &theme);
        assert_eq!(d.plot_area.width, 500);
        assert!(d.plot_area.y >= l.pad + 10, "{:?}", d.plot_area);
        assert!(d.scale_bar.is_some());
        assert!(d.figure.width <= 600.0 && d.figure.height <= 600.0);
    }
}
