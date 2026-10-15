//! Rasterize a [`Figure`] with tiny-skia: anti-aliased paths, text as glyph outlines, images
//! copied pixel for pixel when they sit on whole pixels at their own size.

use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Pixmap, PixmapPaint, StrokeDash,
    Transform,
};

use crate::color::Color;
use crate::font;
use crate::geom::Rect;
use crate::path::Path;
use crate::raster::Raster;
use crate::scene::{Cap, Figure, Item, Stroke};

/// Largest raster side drawn (larger requests are clamped).
const MAX_SIDE: f32 = 16_384.0;

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p.anti_alias = true;
    p
}

fn skia_stroke(s: &Stroke, k: f32) -> tiny_skia::Stroke {
    tiny_skia::Stroke {
        width: (s.width * k).max(0.0),
        miter_limit: 4.0,
        line_cap: match s.cap {
            Cap::Butt => LineCap::Butt,
            Cap::Round => LineCap::Round,
            Cap::Square => LineCap::Square,
        },
        line_join: LineJoin::Round,
        dash: if s.dash.len() >= 2 && s.dash.iter().all(|d| d.is_finite() && *d >= 0.0) {
            StrokeDash::new(s.dash.iter().map(|d| d * k).collect(), 0.0)
        } else {
            None
        },
    }
}

/// Masks for clip rectangles, made once per distinct rectangle.
struct Clips {
    w: u32,
    h: u32,
    k: f32,
    masks: Vec<(Rect, Mask)>,
}

impl Clips {
    fn get(&mut self, r: Rect) -> Option<&Mask> {
        if let Some(i) = self.masks.iter().position(|(c, _)| *c == r) {
            return Some(&self.masks[i].1);
        }
        let mut m = Mask::new(self.w, self.h)?;
        let p = Path::rect(r.x, r.y, r.w.max(0.01), r.h.max(0.01)).to_skia(self.k)?;
        m.fill_path(&p, FillRule::Winding, true, Transform::identity());
        self.masks.push((r, m));
        self.masks.last().map(|(_, m)| m)
    }
}

/// Render `fig` at `scale` into an RGB raster (transparent areas over white).
pub(crate) fn raster(fig: &Figure, scale: f32) -> Raster {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let w = (fig.width * k).round().clamp(1.0, MAX_SIDE) as u32;
    let h = (fig.height * k).round().clamp(1.0, MAX_SIDE) as u32;
    let Some(mut pm) = Pixmap::new(w, h) else {
        return Raster::new(w, h, [255, 255, 255]);
    };
    let bg = fig.background.unwrap_or(Color::rgb(255, 255, 255));
    pm.fill(tiny_skia::Color::from_rgba8(bg.r, bg.g, bg.b, bg.a));
    let mut clips = Clips {
        w,
        h,
        k,
        masks: Vec::new(),
    };
    for item in &fig.items {
        match item {
            Item::Shape {
                path,
                fill,
                stroke,
                clip,
            } => {
                let Some(sp) = path.to_skia(k) else {
                    continue;
                };
                // A clip only matters when the shape reaches outside it.
                let clip = clip.filter(|c| {
                    path.bounds().is_none_or(|(x0, y0, x1, y1)| {
                        let pad = stroke.as_ref().map_or(0.0, |s| s.width);
                        !c.contains_rect(&Rect::from_corners(
                            x0 - pad,
                            y0 - pad,
                            x1 + pad,
                            y1 + pad,
                        ))
                    })
                });
                let mask = match clip {
                    Some(c) => match clips.get(c) {
                        Some(m) => Some(m),
                        None => continue,
                    },
                    None => None,
                };
                if let Some(c) = fill {
                    pm.fill_path(
                        &sp,
                        &paint(*c),
                        FillRule::Winding,
                        Transform::identity(),
                        mask,
                    );
                }
                if let Some(s) = stroke {
                    pm.stroke_path(
                        &sp,
                        &paint(s.color),
                        &skia_stroke(s, k),
                        Transform::identity(),
                        mask,
                    );
                }
            }
            Item::Text { x, y, text, style } => {
                let (dx, dy) = style.offset(text);
                let glyphs = font::text_path(text, style.face, style.size, x + dx, y + dy);
                let Some(sp) = glyphs.to_skia(k) else {
                    continue;
                };
                let t = if style.rotate == 0.0 {
                    Transform::identity()
                } else {
                    Transform::from_rotate_at(style.rotate, x * k, y * k)
                };
                if let Some((c, hw)) = style.halo {
                    let s = tiny_skia::Stroke {
                        width: hw * 2.0 * k,
                        line_join: LineJoin::Round,
                        ..tiny_skia::Stroke::default()
                    };
                    pm.stroke_path(&sp, &paint(c), &s, t, None);
                }
                pm.fill_path(&sp, &paint(style.color), FillRule::Winding, t, None);
            }
            Item::Image {
                rect,
                raster,
                smooth,
            } => draw_image(&mut pm, *rect, raster, *smooth, k),
        }
    }
    // Composite over the background colour (opaque already unless the background was not).
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for px in pm.data().as_chunks::<4>().0 {
        let a = u32::from(px[3]);
        for c in &px[..3] {
            // premultiplied over white
            rgb.push((u32::from(*c) + (255 - a)).min(255) as u8);
        }
    }
    Raster {
        width: w,
        height: h,
        rgb,
    }
}

fn draw_image(pm: &mut Pixmap, rect: Rect, r: &Raster, smooth: bool, k: f32) {
    if r.width == 0 || r.height == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let (x, y) = (rect.x * k, rect.y * k);
    let (sx, sy) = (rect.w * k / r.width as f32, rect.h * k / r.height as f32);
    let exact =
        (sx - 1.0).abs() < 1e-6 && (sy - 1.0).abs() < 1e-6 && x.fract() == 0.0 && y.fract() == 0.0;
    if exact {
        // One raster pixel per output pixel: copy, so the data reaches the output unchanged.
        let (pw, ph) = (pm.width() as i64, pm.height() as i64);
        let data = pm.data_mut();
        for row in 0..i64::from(r.height) {
            let oy = y as i64 + row;
            if oy < 0 || oy >= ph {
                continue;
            }
            for col in 0..i64::from(r.width) {
                let ox = x as i64 + col;
                if ox < 0 || ox >= pw {
                    continue;
                }
                let s = ((row * i64::from(r.width) + col) * 3) as usize;
                let d = ((oy * pw + ox) * 4) as usize;
                data[d..d + 3].copy_from_slice(&r.rgb[s..s + 3]);
                data[d + 3] = 255;
            }
        }
        return;
    }
    let Some(mut src) = Pixmap::new(r.width, r.height) else {
        return;
    };
    for (d, s) in src
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(r.rgb.as_chunks::<3>().0)
    {
        *d = [s[0], s[1], s[2], 255];
    }
    let paint = PixmapPaint {
        quality: if smooth {
            FilterQuality::Bicubic
        } else {
            FilterQuality::Nearest
        },
        ..PixmapPaint::default()
    };
    let t = Transform::from_translate(x, y).pre_scale(sx, sy);
    pm.draw_pixmap(0, 0, src.as_ref(), &paint, t, None);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::scene::TextStyle;

    #[test]
    fn draws_shapes_text_and_exact_images() {
        let mut f = Figure::new(40.0, 20.0, Some(Color::rgb(255, 255, 255)));
        f.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(0, 0, 0));
        let img = Raster {
            width: 2,
            height: 1,
            rgb: vec![1, 2, 3, 4, 5, 6],
        };
        f.image(Rect::new(20.0, 5.0, 2.0, 1.0), Arc::new(img), false);
        f.text(25.0, 15.0, "Hi", &TextStyle::new(10.0, Color::rgb(0, 0, 0)));
        let r = f.to_raster(1.0);
        assert_eq!((r.width, r.height), (40, 20));
        assert_eq!(r.get(5, 5), Some([0, 0, 0]));
        assert_eq!(r.get(20, 5), Some([1, 2, 3]));
        assert_eq!(r.get(21, 5), Some([4, 5, 6]));
        assert_eq!(r.get(15, 15), Some([255, 255, 255]));
        // the text left ink somewhere in its box
        let inked = (25..38).any(|x| (7..15).any(|y| r.get(x, y).unwrap()[0] < 128));
        assert!(inked);
        let r2 = f.to_raster(2.0);
        assert_eq!((r2.width, r2.height), (80, 40));
        // same input, same bytes
        assert_eq!(f.to_png(1.0).unwrap(), f.to_png(1.0).unwrap());
    }

    #[test]
    fn clips_hold() {
        let mut f = Figure::new(20.0, 20.0, None);
        f.shape(
            Path::rect(0.0, 0.0, 20.0, 20.0),
            Some(Color::rgb(0, 0, 0)),
            None,
            Some(Rect::new(0.0, 0.0, 10.0, 20.0)),
        );
        let r = f.to_raster(1.0);
        assert_eq!(r.get(2, 2), Some([0, 0, 0]));
        assert_eq!(r.get(15, 2), Some([255, 255, 255]));
    }
}
