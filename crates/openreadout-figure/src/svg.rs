//! Write a [`Figure`] as an SVG document. Text stays text (set in IBM Plex, with common fallbacks),
//! so it can be selected and edited; raster images are embedded as PNG.

use std::fmt::Write as _;

use crate::color::Color;
use crate::geom::Rect;
use crate::path::fmt_coord;
use crate::scene::{Anchor, Cap, Figure, Item, Stroke};

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c if (c as u32) < 0x20 => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

fn color_attr(name: &str, c: Color) -> String {
    if c.a == 255 {
        format!(" {name}=\"{}\"", c.to_hex())
    } else {
        format!(
            " {name}=\"{}\" {name}-opacity=\"{}\"",
            c.to_hex(),
            fmt_coord(c.opacity())
        )
    }
}

fn stroke_attrs(s: &Stroke) -> String {
    let mut a = color_attr("stroke", s.color);
    let _ = write!(a, " stroke-width=\"{}\"", fmt_coord(s.width));
    a.push_str(" stroke-linejoin=\"round\"");
    match s.cap {
        Cap::Butt => {}
        Cap::Round => a.push_str(" stroke-linecap=\"round\""),
        Cap::Square => a.push_str(" stroke-linecap=\"square\""),
    }
    if s.dash.len() >= 2 {
        let d: Vec<String> = s.dash.iter().map(|v| fmt_coord(*v)).collect();
        let _ = write!(a, " stroke-dasharray=\"{}\"", d.join(" "));
    }
    a
}

/// Standard base64 (RFC 4648, padded).
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let b = [
            ch[0],
            ch.get(1).copied().unwrap_or(0),
            ch.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if ch.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        o.push(if ch.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    o
}

pub(crate) fn svg(fig: &Figure) -> String {
    let mut clips: Vec<Rect> = Vec::new();
    for it in &fig.items {
        if let Item::Shape { clip: Some(c), .. } = it
            && !clips.contains(c)
        {
            clips.push(*c);
        }
    }
    let (w, h) = (fmt_coord(fig.width), fmt_coord(fig.height));
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
    );
    if let Some(t) = &fig.title {
        let _ = write!(s, "<title>{}</title>", esc(t));
    }
    if !clips.is_empty() {
        s.push_str("<defs>");
        for (i, c) in clips.iter().enumerate() {
            let _ = write!(
                s,
                "<clipPath id=\"c{i}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
                fmt_coord(c.x),
                fmt_coord(c.y),
                fmt_coord(c.w),
                fmt_coord(c.h)
            );
        }
        s.push_str("</defs>");
    }
    if let Some(bg) = fig.background {
        let _ = write!(
            s,
            "<rect width=\"100%\" height=\"100%\"{}/>",
            color_attr("fill", bg)
        );
    }
    for it in &fig.items {
        match it {
            Item::Shape {
                path,
                fill,
                stroke,
                clip,
            } => {
                let _ = write!(s, "<path d=\"{}\"", path.to_svg_data());
                match fill {
                    Some(c) => s.push_str(&color_attr("fill", *c)),
                    None => s.push_str(" fill=\"none\""),
                }
                if let Some(st) = stroke {
                    s.push_str(&stroke_attrs(st));
                }
                if let Some(c) = clip
                    && let Some(i) = clips.iter().position(|x| x == c)
                {
                    let _ = write!(s, " clip-path=\"url(#c{i})\"");
                }
                s.push_str("/>");
            }
            Item::Text { x, y, text, style } => {
                let (_, dy) = style.offset(text);
                let anchor = match style.anchor {
                    Anchor::Start => "",
                    Anchor::Middle => " text-anchor=\"middle\"",
                    Anchor::End => " text-anchor=\"end\"",
                };
                let _ = write!(
                    s,
                    "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\"{}",
                    fmt_coord(*x),
                    fmt_coord(y + dy),
                    style.face.css_family(),
                    fmt_coord(style.size),
                    anchor
                );
                if style.face.css_weight() != 400 {
                    let _ = write!(s, " font-weight=\"{}\"", style.face.css_weight());
                }
                s.push_str(&color_attr("fill", style.color));
                if let Some((c, hw)) = style.halo {
                    s.push_str(&color_attr("stroke", c));
                    let _ = write!(
                        s,
                        " stroke-width=\"{}\" stroke-linejoin=\"round\" paint-order=\"stroke\"",
                        fmt_coord(hw * 2.0)
                    );
                }
                if style.rotate != 0.0 {
                    let _ = write!(
                        s,
                        " transform=\"rotate({} {} {})\"",
                        fmt_coord(style.rotate),
                        fmt_coord(*x),
                        fmt_coord(*y)
                    );
                }
                let _ = write!(s, ">{}</text>", esc(text));
            }
            Item::Image {
                rect,
                raster,
                smooth,
            } => {
                let Ok(png) = raster.to_png() else {
                    continue;
                };
                let _ = write!(
                    s,
                    "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"{} href=\"data:image/png;base64,{}\"/>",
                    fmt_coord(rect.x),
                    fmt_coord(rect.y),
                    fmt_coord(rect.w),
                    fmt_coord(rect.h),
                    if *smooth {
                        ""
                    } else {
                        " style=\"image-rendering:pixelated\""
                    },
                    base64(&png)
                );
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Path;
    use crate::scene::TextStyle;

    #[test]
    fn writes_svg() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let mut f = Figure::new(100.0, 50.0, Some(Color::hex(0xF7F5F0)));
        f.title = Some("a <test>".into());
        f.shape(
            Path::segment(0.0, 0.0, 10.0, 10.0),
            None,
            Some(Stroke::new(Color::rgb(0, 0, 0).alpha(0.5), 2.0).dashed(4.0, 2.0)),
            Some(Rect::new(0.0, 0.0, 5.0, 5.0)),
        );
        f.text(
            10.0,
            20.0,
            "5 µm & more",
            &TextStyle::new(12.0, Color::rgb(0, 0, 0))
                .align(Anchor::Middle, crate::scene::Baseline::Middle)
                .halo(Color::rgb(255, 255, 255), 2.0),
        );
        let s = f.to_svg();
        assert!(s.starts_with("<svg xmlns"));
        assert!(s.contains("<title>a &lt;test&gt;</title>"));
        assert!(s.contains("clip-path=\"url(#c0)\""));
        assert!(s.contains("stroke-dasharray=\"4 2\""));
        assert!(s.contains("stroke-opacity=\"0.5\""));
        assert!(s.contains(">5 µm &amp; more</text>"));
        assert!(s.contains("paint-order=\"stroke\""));
    }
}
