//! Write a [`Figure`] as an SVG document. Text stays text (set in IBM Plex, with common fallbacks),
//! so it can be selected and edited; raster images are embedded as PNG.

use std::fmt::Write as _;

use crate::color::Color;
use crate::geom::Rect;
use crate::path::fmt_coord;
use crate::scene::{Anchor, Cap, Figure, Item, Layer, Stroke};

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

/// Stands for the document's id suffix until the whole document is written and hashed. Escaped
/// text turns control characters into spaces, so this character only appears in ids.
const ID_SUFFIX: char = '\u{1}';

/// 32-bit FNV-1a folded from 64 bits: a short, stable fingerprint of the document.
fn fingerprint(s: &str) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ((h >> 32) ^ (h & 0xffff_ffff)) as u32
}

/// A readable id stem from a group's layer and name: lower-case letters and digits, with
/// hyphens between words, led by the class unless the name already starts or ends with it
/// ("series-signal", but "x-axis" and "panel-1").
fn slug(class: &str, name: &str) -> String {
    let mut s = String::new();
    let mut gap = false;
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            if gap && !s.is_empty() {
                s.push('-');
            }
            s.push(c);
            gap = false;
        } else {
            gap = true;
        }
        if s.len() >= 40 {
            break;
        }
    }
    if s.is_empty() {
        class.to_string()
    } else if s.starts_with(class) || s.ends_with(class) {
        s
    } else {
        format!("{class}-{s}")
    }
}

/// The `<g>` tags of the figure's groups: which open before item `i` and where each closes.
struct Groups<'a> {
    fig: &'a Figure,
    next: usize,
    /// End index of each open group, innermost last.
    open: Vec<usize>,
    used: Vec<String>,
}

impl Groups<'_> {
    /// Close the groups that end at or before item `i`.
    fn close(&mut self, s: &mut String, i: usize) {
        while self.open.last().is_some_and(|&e| e <= i) {
            self.open.pop();
            s.push_str("</g>");
        }
    }

    /// Open the groups that start at item `i`. A group that is empty, out of order, or that
    /// would cross its parent's end is cut to fit or left out, so the tags always nest.
    fn open(&mut self, s: &mut String, i: usize) {
        let n = self.fig.items.len();
        while let Some(g) = self.fig.groups.get(self.next) {
            if g.start > i {
                break;
            }
            self.next += 1;
            let end = g.end.min(n).min(self.open.last().copied().unwrap_or(n));
            if g.start < i || end <= i {
                continue;
            }
            let class = g.layer.class();
            let stem = slug(class, &g.name);
            let mut id = stem.clone();
            let mut k = 2;
            while self.used.contains(&id) {
                id = format!("{stem}-{k}");
                k += 1;
            }
            let label = if g.name.is_empty() { class } else { &g.name };
            let _ = write!(
                s,
                "<g id=\"{id}{ID_SUFFIX}\" class=\"{class}\" inkscape:label=\"{}\"",
                esc(label)
            );
            self.used.push(id);
            if self.open.is_empty() {
                s.push_str(" inkscape:groupmode=\"layer\"");
            }
            if g.layer == Layer::Series {
                let _ = write!(s, " data-series=\"{}\"", esc(&g.name));
            }
            s.push('>');
            self.open.push(end);
        }
    }
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
    let mut s = String::new();
    if let Some(t) = &fig.title {
        let _ = write!(s, "<title>{}</title>", esc(t));
    }
    if !clips.is_empty() {
        s.push_str("<defs>");
        for (i, c) in clips.iter().enumerate() {
            let _ = write!(
                s,
                "<clipPath id=\"clip{i}{ID_SUFFIX}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
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
    let mut groups = Groups {
        fig,
        next: 0,
        open: Vec::new(),
        used: Vec::new(),
    };
    for (i, it) in fig.items.iter().enumerate() {
        groups.close(&mut s, i);
        groups.open(&mut s, i);
        item(&mut s, it, &clips);
    }
    groups.close(&mut s, usize::MAX);
    let ns = if groups.used.is_empty() {
        ""
    } else {
        " xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\""
    };
    let doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"{ns} width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">{s}</svg>\n"
    );
    // Ids end in a fingerprint of the document, so two figures inlined in one HTML page keep
    // their clip paths apart.
    let suffix = format!("-{:08x}", fingerprint(&doc));
    doc.replace(ID_SUFFIX, &suffix)
}

/// Write one item.
fn item(s: &mut String, it: &Item, clips: &[Rect]) {
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
                let _ = write!(s, " clip-path=\"url(#clip{i}{ID_SUFFIX})\"");
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
                return;
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
        let suffix = format!("-{:08x}", fingerprint(&s.replace(&id_suffix(&s), "\u{1}")));
        assert!(s.contains(&format!("<clipPath id=\"clip0{suffix}\">")));
        assert!(s.contains(&format!("clip-path=\"url(#clip0{suffix})\"")));
        assert!(s.contains("stroke-dasharray=\"4 2\""));
        assert!(s.contains("stroke-opacity=\"0.5\""));
        assert!(s.contains(">5 µm &amp; more</text>"));
        assert!(s.contains("paint-order=\"stroke\""));
        // no groups, no Inkscape namespace
        assert!(!s.contains("<g") && !s.contains("inkscape"));
    }

    /// The id suffix a document uses, read from its first clip path.
    fn id_suffix(s: &str) -> String {
        let at = s.find("id=\"clip0-").unwrap() + "id=\"clip0".len();
        s[at..at + 9].to_string()
    }

    fn two_series() -> Figure {
        let mut f = Figure::new(100.0, 50.0, None);
        let clip = Some(Rect::new(0.0, 0.0, 90.0, 40.0));
        let ink = Stroke::new(Color::rgb(0, 0, 0), 1.0);
        f.group(Layer::Panel, "Run 1", |f| {
            f.group(Layer::Axis, "x axis", |f| {
                f.line(0.0, 40.0, 90.0, 40.0, ink.clone());
            });
            for name in ["signal", "signal", "ref & <b>"] {
                f.group(Layer::Series, name, |f| {
                    f.shape(
                        Path::segment(0.0, 0.0, 90.0, 40.0),
                        None,
                        Some(ink.clone()),
                        clip,
                    );
                });
            }
            // an empty group writes nothing
            f.group(Layer::Annotation, "nothing", |_| {});
        });
        f.group(Layer::Caption, "", |f| {
            f.text(
                0.0,
                48.0,
                "caption",
                &TextStyle::new(8.0, Color::rgb(0, 0, 0)),
            );
        });
        f
    }

    #[test]
    fn writes_groups() {
        let s = two_series().to_svg();
        let x = id_suffix(&s);
        assert!(s.contains("xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\""));
        assert!(s.contains(&format!(
            "<g id=\"panel-run-1{x}\" class=\"panel\" inkscape:label=\"Run 1\" inkscape:groupmode=\"layer\">"
        )));
        assert!(s.contains(&format!(
            "<g id=\"x-axis{x}\" class=\"axis\" inkscape:label=\"x axis\"><path"
        )));
        assert!(s.contains(&format!(
            "<g id=\"series-signal{x}\" class=\"series\" inkscape:label=\"signal\" data-series=\"signal\">"
        )));
        assert!(s.contains(&format!("<g id=\"series-signal-2{x}\"")));
        assert!(s.contains(&format!(
            "<g id=\"series-ref-b{x}\" class=\"series\" inkscape:label=\"ref &amp; &lt;b&gt;\" data-series=\"ref &amp; &lt;b&gt;\">"
        )));
        assert!(!s.contains("annotation"));
        assert!(s.contains(&format!(
            "<g id=\"caption{x}\" class=\"caption\" inkscape:label=\"caption\" inkscape:groupmode=\"layer\"><text"
        )));
        assert_eq!(s.matches("<g ").count(), s.matches("</g>").count());
        assert_eq!(s.matches("<g ").count(), 6);
        // the panel closes after its last series, before the caption opens
        assert!(s.contains("/></g></g><g id=\"caption"));
    }

    #[test]
    fn ids_differ_between_figures() {
        let a = two_series();
        let mut b = two_series();
        b.width = 120.0;
        let (sa, sb) = (a.to_svg(), b.to_svg());
        assert_eq!(sa, a.to_svg(), "same figure, same bytes");
        assert_ne!(id_suffix(&sa), id_suffix(&sb));
        assert!(!sa.contains('\u{1}'));
    }

    #[test]
    fn bad_groups_still_nest() {
        let mut f = two_series();
        // a group that crosses its parent's end, one that ends before it starts, and one
        // past the last item
        f.groups.insert(
            1,
            crate::scene::Group {
                layer: Layer::Grid,
                name: "cross".into(),
                start: 0,
                end: 99,
            },
        );
        f.groups.push(crate::scene::Group {
            layer: Layer::Grid,
            name: "backwards".into(),
            start: 4,
            end: 2,
        });
        let s = f.to_svg();
        assert_eq!(s.matches("<g ").count(), s.matches("</g>").count());
        assert!(!s.contains("backwards"));
    }

    #[test]
    fn groups_leave_rasters_alone() {
        let mut flat = two_series();
        flat.groups.clear();
        assert_eq!(flat.to_raster(1.0), two_series().to_raster(1.0));
    }
}
