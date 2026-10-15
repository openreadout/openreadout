//! The figure: an ordered list of shapes, text and raster images in pixel coordinates (y down),
//! drawn in order. Build it with the methods here or with the chart helpers, then render it with
//! [`Figure::to_raster`], [`Figure::to_png`], [`Figure::to_jpeg`] or [`Figure::to_svg`].

use std::sync::Arc;

use openreadout_core::Result;

use crate::color::Color;
use crate::font::{self, Face};
use crate::geom::Rect;
use crate::path::Path;
use crate::raster::Raster;

/// How a line ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cap {
    /// Flat at the end point.
    #[default]
    Butt,
    /// Rounded past the end point.
    Round,
    /// Square past the end point.
    Square,
}

/// A stroke: colour, width and dash pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// Colour (with opacity).
    pub color: Color,
    /// Width in pixels.
    pub width: f32,
    /// Dash lengths (on, off, …); empty for a solid line.
    pub dash: Vec<f32>,
    /// Line ends.
    pub cap: Cap,
}

impl Stroke {
    /// A solid stroke with round joins.
    pub fn new(color: Color, width: f32) -> Stroke {
        Stroke {
            color,
            width,
            dash: Vec::new(),
            cap: Cap::Butt,
        }
    }

    /// The same stroke, dashed.
    pub fn dashed(mut self, on: f32, off: f32) -> Stroke {
        self.dash = vec![on, off];
        self
    }

    /// The same stroke with round ends.
    pub fn round(mut self) -> Stroke {
        self.cap = Cap::Round;
        self
    }
}

/// Horizontal text alignment relative to the anchor point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Text starts at the point.
    #[default]
    Start,
    /// Text is centred on the point.
    Middle,
    /// Text ends at the point.
    End,
}

/// Vertical text alignment relative to the anchor point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Baseline {
    /// The point is on the baseline.
    #[default]
    Alphabetic,
    /// The point is halfway up the capitals.
    Middle,
    /// The point is at the top of the capitals.
    Top,
    /// The point is at the bottom of the descenders.
    Bottom,
}

/// How text is set.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// Typeface.
    pub face: Face,
    /// Size in pixels (the em).
    pub size: f32,
    /// Colour.
    pub color: Color,
    /// Horizontal alignment.
    pub anchor: Anchor,
    /// Vertical alignment.
    pub baseline: Baseline,
    /// A halo (outline) under the glyphs in this colour and width, so text reads on any image.
    pub halo: Option<(Color, f32)>,
    /// Rotation in degrees, clockwise, about the anchor point (-90 reads bottom to top).
    pub rotate: f32,
}

impl TextStyle {
    /// Sans text of `size` in `color`, starting at the point, on the baseline.
    pub fn new(size: f32, color: Color) -> TextStyle {
        TextStyle {
            face: Face::Sans,
            size,
            color,
            anchor: Anchor::Start,
            baseline: Baseline::Alphabetic,
            halo: None,
            rotate: 0.0,
        }
    }

    /// Set the face.
    pub fn face(mut self, face: Face) -> TextStyle {
        self.face = face;
        self
    }

    /// Set the alignment.
    pub fn align(mut self, anchor: Anchor, baseline: Baseline) -> TextStyle {
        self.anchor = anchor;
        self.baseline = baseline;
        self
    }

    /// Add a halo.
    pub fn halo(mut self, color: Color, width: f32) -> TextStyle {
        self.halo = Some((color, width));
        self
    }

    /// Rotate (degrees, clockwise).
    pub fn rotate(mut self, degrees: f32) -> TextStyle {
        self.rotate = degrees;
        self
    }

    /// Width of `text` in this style.
    pub fn width(&self, text: &str) -> f32 {
        font::text_width(text, self.face, self.size)
    }

    /// Shift from the anchor point to the start of the baseline, before rotation.
    pub fn offset(&self, text: &str) -> (f32, f32) {
        let w = self.width(text);
        let m = font::metrics(self.face, self.size);
        let dx = match self.anchor {
            Anchor::Start => 0.0,
            Anchor::Middle => -w / 2.0,
            Anchor::End => -w,
        };
        let dy = match self.baseline {
            Baseline::Alphabetic => 0.0,
            Baseline::Middle => m.cap_height / 2.0,
            Baseline::Top => m.cap_height,
            Baseline::Bottom => -m.descent,
        };
        (dx, dy)
    }

    /// The box the text occupies (unrotated): from the cap height to the descender.
    pub fn bounds(&self, x: f32, y: f32, text: &str) -> Rect {
        let (dx, dy) = self.offset(text);
        let m = font::metrics(self.face, self.size);
        let w = self.width(text);
        if self.rotate.abs() > 45.0 && self.rotate.abs() < 135.0 {
            // a vertical label: the box turns about the anchor
            let h = m.cap_height + m.descent;
            let (along0, across0) = (dx, dy - m.cap_height);
            let s = self.rotate.signum();
            // rotating by ±90°: (u, v) -> (-s v, s u)
            let xs = [-s * across0, -s * (across0 + h)];
            let ys = [s * along0, s * (along0 + w)];
            return Rect::new(x + xs[0].min(xs[1]), y + ys[0].min(ys[1]), h, w);
        }
        Rect::new(x + dx, y + dy - m.cap_height, w, m.cap_height + m.descent)
    }
}

/// One thing drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A path, filled and/or stroked, optionally clipped to a rectangle.
    Shape {
        /// The outline.
        path: Path,
        /// Fill colour.
        fill: Option<Color>,
        /// Stroke.
        stroke: Option<Stroke>,
        /// Clip rectangle.
        clip: Option<Rect>,
    },
    /// A line of text at a point.
    Text {
        /// Anchor point.
        x: f32,
        /// Anchor point.
        y: f32,
        /// The text (one line).
        text: String,
        /// How it is set.
        style: TextStyle,
    },
    /// A raster image stretched over a rectangle.
    Image {
        /// Where it goes.
        rect: Rect,
        /// The pixels.
        raster: Arc<Raster>,
        /// Interpolate when scaling (else nearest pixel, which keeps pixels crisp).
        smooth: bool,
    },
}

/// A figure: a canvas size, a background and the items drawn on it, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    /// Width in pixels at scale 1.
    pub width: f32,
    /// Height in pixels at scale 1.
    pub height: f32,
    /// Background (`None`: transparent in SVG, white in rasters).
    pub background: Option<Color>,
    /// What is drawn, first to last.
    pub items: Vec<Item>,
    /// A short description for SVG `<title>` and accessibility.
    pub title: Option<String>,
}

impl Figure {
    /// An empty figure.
    pub fn new(width: f32, height: f32, background: Option<Color>) -> Figure {
        Figure {
            width: width.max(1.0),
            height: height.max(1.0),
            background,
            items: Vec::new(),
            title: None,
        }
    }

    /// The whole figure as a rectangle.
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }

    /// Add any item.
    pub fn push(&mut self, item: Item) -> &mut Self {
        self.items.push(item);
        self
    }

    /// Fill a path.
    pub fn fill(&mut self, path: Path, color: Color) -> &mut Self {
        self.shape(path, Some(color), None, None)
    }

    /// Stroke a path.
    pub fn stroke(&mut self, path: Path, stroke: Stroke) -> &mut Self {
        self.shape(path, None, Some(stroke), None)
    }

    /// Fill and/or stroke a path, optionally clipped.
    pub fn shape(
        &mut self,
        path: Path,
        fill: Option<Color>,
        stroke: Option<Stroke>,
        clip: Option<Rect>,
    ) -> &mut Self {
        if !path.is_empty() && (fill.is_some() || stroke.is_some()) {
            self.items.push(Item::Shape {
                path,
                fill,
                stroke,
                clip,
            });
        }
        self
    }

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, r: Rect, color: Color) -> &mut Self {
        self.fill(Path::rect(r.x, r.y, r.w, r.h), color)
    }

    /// A straight line.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stroke: Stroke) -> &mut Self {
        self.stroke(Path::segment(x0, y0, x1, y1), stroke)
    }

    /// A line of text. Returns the box it occupies.
    pub fn text(&mut self, x: f32, y: f32, text: &str, style: &TextStyle) -> Rect {
        let b = style.bounds(x, y, text);
        if !text.is_empty() && x.is_finite() && y.is_finite() {
            self.items.push(Item::Text {
                x,
                y,
                text: text.to_string(),
                style: style.clone(),
            });
        }
        b
    }

    /// A raster image over `rect`.
    pub fn image(&mut self, rect: Rect, raster: Arc<Raster>, smooth: bool) -> &mut Self {
        self.items.push(Item::Image {
            rect,
            raster,
            smooth,
        });
        self
    }

    /// Render at `scale` (1 = one pixel per figure unit).
    pub fn to_raster(&self, scale: f32) -> Raster {
        crate::render::raster(self, scale)
    }

    /// PNG at `scale`.
    pub fn to_png(&self, scale: f32) -> Result<Vec<u8>> {
        self.to_raster(scale).to_png()
    }

    /// JPEG at `scale` and `quality`.
    pub fn to_jpeg(&self, scale: f32, quality: u8) -> Result<Vec<u8>> {
        self.to_raster(scale).to_jpeg(quality)
    }

    /// SVG document text.
    pub fn to_svg(&self) -> String {
        crate::svg::svg(self)
    }
}
