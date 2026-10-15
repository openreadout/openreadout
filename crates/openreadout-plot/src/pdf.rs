//! A figure as a one-page vector PDF: paths are filled and stroked as PDF paths, text is drawn
//! as glyph outlines (so no font is embedded and the look is exact), raster images become
//! Flate-compressed image objects. One figure unit is `points_per_unit` PDF points (1 unless a
//! print size is asked for).

use std::fmt::Write as _;

use openreadout_figure::{Cap, Color, Figure, Item, Path, Segment, text_path};

struct Doc {
    objects: Vec<Vec<u8>>,
}

impl Doc {
    fn add(&mut self, body: Vec<u8>) -> usize {
        self.objects.push(body);
        self.objects.len()
    }
}

fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

fn rgb(c: Color) -> String {
    format!(
        "{} {} {}",
        num(f32::from(c.r) / 255.0),
        num(f32::from(c.g) / 255.0),
        num(f32::from(c.b) / 255.0)
    )
}

/// Path operators; `h` is the page height (PDF y runs up).
fn path_ops(out: &mut String, p: &Path, h: f32, rot: Option<(f32, f32, f32)>) {
    let tf = |x: f32, y: f32| -> (f32, f32) {
        let (x, y) = match rot {
            Some((deg, cx, cy)) => {
                let (s, c) = deg.to_radians().sin_cos();
                let (dx, dy) = (x - cx, y - cy);
                (cx + dx * c - dy * s, cy + dx * s + dy * c)
            }
            None => (x, y),
        };
        (x, h - y)
    };
    let mut cur = (0.0f32, 0.0f32);
    for s in p.segments() {
        match *s {
            Segment::MoveTo(x, y) => {
                let (a, b) = tf(x, y);
                let _ = write!(out, "{} {} m ", num(a), num(b));
                cur = (x, y);
            }
            Segment::LineTo(x, y) => {
                let (a, b) = tf(x, y);
                let _ = write!(out, "{} {} l ", num(a), num(b));
                cur = (x, y);
            }
            Segment::QuadTo(x1, y1, x, y) => {
                // as a cubic
                let c1 = (
                    cur.0 + 2.0 / 3.0 * (x1 - cur.0),
                    cur.1 + 2.0 / 3.0 * (y1 - cur.1),
                );
                let c2 = (x + 2.0 / 3.0 * (x1 - x), y + 2.0 / 3.0 * (y1 - y));
                let (a1, b1) = tf(c1.0, c1.1);
                let (a2, b2) = tf(c2.0, c2.1);
                let (a, b) = tf(x, y);
                let _ = write!(
                    out,
                    "{} {} {} {} {} {} c ",
                    num(a1),
                    num(b1),
                    num(a2),
                    num(b2),
                    num(a),
                    num(b)
                );
                cur = (x, y);
            }
            Segment::CubicTo(x1, y1, x2, y2, x, y) => {
                let (a1, b1) = tf(x1, y1);
                let (a2, b2) = tf(x2, y2);
                let (a, b) = tf(x, y);
                let _ = write!(
                    out,
                    "{} {} {} {} {} {} c ",
                    num(a1),
                    num(b1),
                    num(a2),
                    num(b2),
                    num(a),
                    num(b)
                );
                cur = (x, y);
            }
            Segment::Close => out.push_str("h "),
        }
    }
}

/// The figure as PDF bytes, one figure unit to `points_per_unit` points.
pub(crate) fn pdf(fig: &Figure, points_per_unit: f32) -> Vec<u8> {
    let (w, h) = (fig.width, fig.height);
    let k = if points_per_unit.is_finite() && points_per_unit > 0.0 {
        points_per_unit
    } else {
        1.0
    };
    let mut doc = Doc {
        objects: Vec::new(),
    };
    let mut content = String::new();
    if k != 1.0 {
        let _ = writeln!(content, "{k:.6} 0 0 {k:.6} 0 0 cm");
    }
    let mut alphas: Vec<u8> = Vec::new();
    let mut images: Vec<usize> = Vec::new();
    let gs = |a: u8, alphas: &mut Vec<u8>| -> String {
        let i = alphas.iter().position(|&x| x == a).unwrap_or_else(|| {
            alphas.push(a);
            alphas.len() - 1
        });
        format!("/A{i} gs ")
    };
    if let Some(bg) = fig.background {
        let _ = writeln!(content, "{} rg 0 0 {} {} re f", rgb(bg), num(w), num(h));
    }
    for item in &fig.items {
        match item {
            Item::Shape {
                path,
                fill,
                stroke,
                clip,
            } => {
                content.push_str("q ");
                if let Some(r) = clip {
                    let _ = write!(
                        content,
                        "{} {} {} {} re W n ",
                        num(r.x),
                        num(h - r.bottom()),
                        num(r.w),
                        num(r.h)
                    );
                }
                if let Some(c) = fill {
                    content.push_str(&gs(c.a, &mut alphas));
                    let _ = write!(content, "{} rg ", rgb(*c));
                    path_ops(&mut content, path, h, None);
                    content.push_str("f\n");
                }
                if let Some(s) = stroke {
                    content.push_str(&gs(s.color.a, &mut alphas));
                    let cap = match s.cap {
                        Cap::Butt => 0,
                        Cap::Round => 1,
                        Cap::Square => 2,
                    };
                    let _ = write!(
                        content,
                        "{} RG {} w {cap} J 1 j ",
                        rgb(s.color),
                        num(s.width)
                    );
                    if !s.dash.is_empty() {
                        let d: Vec<String> = s.dash.iter().map(|v| num(*v)).collect();
                        let _ = write!(content, "[{}] 0 d ", d.join(" "));
                    }
                    path_ops(&mut content, path, h, None);
                    content.push_str("S\n");
                }
                content.push_str("Q\n");
            }
            Item::Text { x, y, text, style } => {
                let (dx, dy) = style.offset(text);
                let glyphs = text_path(text, style.face, style.size, x + dx, y + dy);
                let rot = (style.rotate != 0.0).then_some((style.rotate, *x, *y));
                content.push_str("q ");
                if let Some((c, hw)) = style.halo {
                    content.push_str(&gs(c.a, &mut alphas));
                    let _ = write!(content, "{} RG {} w 1 j 1 J ", rgb(c), num(hw * 2.0));
                    path_ops(&mut content, &glyphs, h, rot);
                    content.push_str("S ");
                }
                content.push_str(&gs(style.color.a, &mut alphas));
                let _ = write!(content, "{} rg ", rgb(style.color));
                path_ops(&mut content, &glyphs, h, rot);
                content.push_str("f Q\n");
            }
            Item::Image {
                rect,
                raster,
                smooth,
            } => {
                let data = miniz_oxide::deflate::compress_to_vec_zlib(&raster.rgb, 6);
                let mut body = format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Interpolate {} /Length {} >>\nstream\n",
                    raster.width,
                    raster.height,
                    smooth,
                    data.len()
                )
                .into_bytes();
                body.extend_from_slice(&data);
                body.extend_from_slice(b"\nendstream");
                let id = doc.add(body);
                images.push(id);
                let _ = writeln!(
                    content,
                    "q {} 0 0 {} {} {} cm /I{} Do Q",
                    num(rect.w),
                    num(rect.h),
                    num(rect.x),
                    num(h - rect.bottom()),
                    images.len() - 1
                );
            }
        }
    }
    let stream = miniz_oxide::deflate::compress_to_vec_zlib(content.as_bytes(), 6);
    let mut body = format!(
        "<< /Length {} /Filter /FlateDecode >>\nstream\n",
        stream.len()
    )
    .into_bytes();
    body.extend_from_slice(&stream);
    body.extend_from_slice(b"\nendstream");
    let content_id = doc.add(body);
    let mut res = String::from("<< /ExtGState << ");
    for (i, a) in alphas.iter().enumerate() {
        let v = num(f32::from(*a) / 255.0);
        let _ = write!(res, "/A{i} << /ca {v} /CA {v} >> ");
    }
    res.push_str(">> /XObject << ");
    for (i, id) in images.iter().enumerate() {
        let _ = write!(res, "/I{i} {id} 0 R ");
    }
    res.push_str(">> >>");
    // pages object is next after the page
    let page_id = doc.objects.len() + 1;
    let pages_id = page_id + 1;
    doc.add(
        format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Contents {content_id} 0 R /Resources {res} >>",
            num(w * k),
            num(h * k)
        )
        .into_bytes(),
    );
    doc.add(format!("<< /Type /Pages /Kids [{page_id} 0 R] /Count 1 >>").into_bytes());
    let title = fig
        .title
        .as_deref()
        .map(|t| {
            let esc: String = t
                .chars()
                .filter(char::is_ascii)
                .map(|c| match c {
                    '(' | ')' | '\\' => format!("\\{c}"),
                    c => c.to_string(),
                })
                .collect();
            format!("/Title ({esc}) ")
        })
        .unwrap_or_default();
    let info_id = doc.add(format!("<< {title}/Producer (OpenReadout) >>").into_bytes());
    let catalog_id = doc.add(format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes());
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in doc.objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", doc.objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {catalog_id} 0 R /Info {info_id} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            doc.objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use openreadout_figure::{Stroke, TextStyle, Theme};

    use super::*;

    #[test]
    fn writes_a_pdf() {
        let t = Theme::light();
        let mut f = Figure::new(200.0, 100.0, Some(t.paper));
        f.line(
            10.0,
            10.0,
            190.0,
            90.0,
            Stroke::new(t.accent.alpha(0.5), 2.0).dashed(4.0, 2.0),
        );
        f.text(20.0, 50.0, "OD600 (λ)", &TextStyle::new(14.0, t.ink));
        let b = pdf(&f, 1.0);
        assert!(b.starts_with(b"%PDF-1.4"));
        assert!(b.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("/MediaBox [0 0 200 100]"));
        assert!(s.contains("/ca 0.5"));
    }
}
