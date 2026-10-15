//! The embedded fonts (IBM Plex Sans regular and medium, IBM Plex Mono), their metrics, kerning
//! and glyph outlines.
//!
//! The fonts ship as the unmodified WOFF files IBM publishes. WOFF wraps the usual font tables,
//! each compressed with zlib, so the first use inflates them back into a font file in memory.
//! Characters a face lacks (Plex Mono has no Greek) are taken from Plex Sans; characters neither
//! has are drawn as an empty advance.

use std::sync::OnceLock;

use ttf_parser::{Face as TtfFace, GlyphId, OutlineBuilder};

use crate::path::Path;

/// A typeface of the figure engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Face {
    /// IBM Plex Sans, regular: labels, ticks, body text.
    #[default]
    Sans,
    /// IBM Plex Sans, medium: titles and emphasis.
    SansMedium,
    /// IBM Plex Mono, regular: coordinates and numbers that should line up.
    Mono,
}

impl Face {
    /// The CSS font stack for this face in SVG output.
    pub fn css_family(self) -> &'static str {
        match self {
            Face::Sans | Face::SansMedium => "'IBM Plex Sans', 'Helvetica Neue', Arial, sans-serif",
            Face::Mono => "'IBM Plex Mono', Menlo, Consolas, monospace",
        }
    }

    /// The CSS font weight for this face in SVG output.
    pub fn css_weight(self) -> u16 {
        match self {
            Face::SansMedium => 500,
            Face::Sans | Face::Mono => 400,
        }
    }

    fn font(self) -> Option<&'static Font> {
        match self {
            Face::Sans => font(&SANS, SANS_WOFF),
            Face::SansMedium => font(&SANS_MEDIUM, SANS_MEDIUM_WOFF),
            Face::Mono => font(&MONO, MONO_WOFF),
        }
    }
}

const SANS_WOFF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.woff");
const SANS_MEDIUM_WOFF: &[u8] = include_bytes!("../fonts/IBMPlexSans-Medium.woff");
const MONO_WOFF: &[u8] = include_bytes!("../fonts/IBMPlexMono-Regular.woff");

static SANS: OnceLock<Option<Font>> = OnceLock::new();
static SANS_MEDIUM: OnceLock<Option<Font>> = OnceLock::new();
static MONO: OnceLock<Option<Font>> = OnceLock::new();

fn font(cell: &'static OnceLock<Option<Font>>, woff: &[u8]) -> Option<&'static Font> {
    cell.get_or_init(|| Font::load(woff)).as_ref()
}

/// A parsed font and the kerning lookups of its `kern` feature.
struct Font {
    face: TtfFace<'static>,
    kern_lookups: Vec<u16>,
    units_per_em: f32,
}

impl Font {
    fn load(woff: &[u8]) -> Option<Font> {
        // The font data lives as long as the process: one copy per face, made once.
        let data: &'static [u8] = Box::leak(woff_to_sfnt(woff)?.into_boxed_slice());
        let face = TtfFace::parse(data, 0).ok()?;
        let mut kern_lookups = Vec::new();
        if let Some(gpos) = face.tables().gpos {
            for feature in gpos.features {
                if feature.tag == ttf_parser::Tag::from_bytes(b"kern") {
                    kern_lookups.extend(feature.lookup_indices);
                }
            }
        }
        kern_lookups.sort_unstable();
        kern_lookups.dedup();
        let units_per_em = f32::from(face.units_per_em().max(1));
        Some(Font {
            face,
            kern_lookups,
            units_per_em,
        })
    }

    fn kern(&self, left: GlyphId, right: GlyphId) -> i16 {
        use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};
        let Some(gpos) = self.face.tables().gpos else {
            return 0;
        };
        for &li in &self.kern_lookups {
            let Some(lookup) = gpos.lookups.get(li) else {
                continue;
            };
            for sub in lookup.subtables.into_iter::<PositioningSubtable>() {
                let PositioningSubtable::Pair(pair) = sub else {
                    continue;
                };
                let Some(ci) = pair.coverage().get(left) else {
                    continue;
                };
                let found = match pair {
                    PairAdjustment::Format1 { sets, .. } => {
                        sets.get(ci).and_then(|set| set.get(right))
                    }
                    PairAdjustment::Format2 {
                        classes, matrix, ..
                    } => matrix.get((classes.0.get(left), classes.1.get(right))),
                };
                if let Some((first, _)) = found {
                    return first.x_advance;
                }
            }
        }
        0
    }
}

/// Inflate a WOFF 1.0 file into the font file it wraps. `None` when it is not a valid WOFF.
fn woff_to_sfnt(w: &[u8]) -> Option<Vec<u8>> {
    let be32 = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes(
            w.get(o..o.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let be16 = |o: usize| -> Option<u16> {
        Some(u16::from_be_bytes(
            w.get(o..o.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    if w.get(0..4)? != b"wOFF" {
        return None;
    }
    let flavor = be32(4)?;
    let n = usize::from(be16(12)?);
    let mut tables: Vec<([u8; 4], u32, Vec<u8>)> = Vec::with_capacity(n);
    for i in 0..n {
        let e = 44 + i * 20;
        let tag: [u8; 4] = w.get(e..e + 4)?.try_into().ok()?;
        let off = be32(e + 4)? as usize;
        let comp = be32(e + 8)? as usize;
        let orig = be32(e + 12)? as usize;
        let checksum = be32(e + 16)?;
        let raw = w.get(off..off.checked_add(comp)?)?;
        let data = if comp < orig {
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw, orig).ok()?
        } else {
            raw.to_vec()
        };
        if data.len() != orig {
            return None;
        }
        tables.push((tag, checksum, data));
    }
    let n16 = u16::try_from(n).ok()?;
    let pow = if n16 == 0 {
        0
    } else {
        15 - n16.leading_zeros() as u16
    };
    let search = 16u16.checked_shl(u32::from(pow)).unwrap_or(0);
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&n16.to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&pow.to_be_bytes());
    out.extend_from_slice(&(n16.saturating_mul(16).saturating_sub(search)).to_be_bytes());
    let mut offset = 12 + 16 * n;
    for (tag, checksum, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(data.len()).ok()?.to_be_bytes());
        offset += data.len().next_multiple_of(4);
    }
    for (_, _, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    Some(out)
}

/// One placed glyph: which font and glyph, and its pen position in pixels from the text start.
#[derive(Clone, Copy)]
struct Placed {
    font: &'static Font,
    glyph: GlyphId,
    x: f32,
}

/// The font that draws `c` for `face`: the face itself, else Plex Sans.
fn glyph_for(face: Face, c: char) -> Option<(&'static Font, GlyphId)> {
    let f = face.font()?;
    if let Some(g) = f.face.glyph_index(c) {
        return Some((f, g));
    }
    let sans = Face::Sans.font()?;
    sans.face.glyph_index(c).map(|g| (sans, g))
}

/// Lay out a line of text: glyph positions and the total advance, in pixels at `size`.
fn layout(text: &str, face: Face, size: f32) -> (Vec<Placed>, f32) {
    let mut out = Vec::with_capacity(text.len());
    let mut x = 0f32;
    let mut prev: Option<(&'static Font, GlyphId)> = None;
    for c in text.chars() {
        let Some((font, glyph)) = glyph_for(face, c) else {
            x += size * 0.5;
            prev = None;
            continue;
        };
        let k = size / font.units_per_em;
        if let Some((pf, pg)) = prev
            && std::ptr::eq(pf, font)
        {
            x += f32::from(font.kern(pg, glyph)) * k;
        }
        out.push(Placed { font, glyph, x });
        x += f32::from(font.face.glyph_hor_advance(glyph).unwrap_or(0)) * k;
        prev = Some((font, glyph));
    }
    (out, x)
}

/// Width in pixels of `text` set in `face` at `size` pixels (kerning applied).
pub fn text_width(text: &str, face: Face, size: f32) -> f32 {
    layout(text, face, size).1
}

/// Vertical metrics of a face at a size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Distance from the baseline up to the top of capitals.
    pub cap_height: f32,
    /// Distance from the baseline up to the top of lowercase x.
    pub x_height: f32,
    /// Distance from the baseline up to the highest ascender.
    pub ascent: f32,
    /// Distance from the baseline down to the lowest descender (positive).
    pub descent: f32,
}

/// Vertical metrics of `face` at `size` pixels.
pub fn metrics(face: Face, size: f32) -> Metrics {
    let fallback = Metrics {
        cap_height: size * 0.7,
        x_height: size * 0.52,
        ascent: size * 1.025,
        descent: size * 0.275,
    };
    let Some(f) = face.font() else {
        return fallback;
    };
    let k = size / f.units_per_em;
    let face = &f.face;
    Metrics {
        cap_height: face
            .capital_height()
            .map_or(fallback.cap_height, |v| f32::from(v) * k),
        x_height: face
            .x_height()
            .map_or(fallback.x_height, |v| f32::from(v) * k),
        ascent: f32::from(face.ascender()) * k,
        descent: -f32::from(face.descender()) * k,
    }
}

struct Outline<'a> {
    path: &'a mut Path,
    x: f32,
    y: f32,
    k: f32,
}

impl OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x * self.k, self.y - y * self.k);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x * self.k, self.y - y * self.k);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(
            self.x + x1 * self.k,
            self.y - y1 * self.k,
            self.x + x * self.k,
            self.y - y * self.k,
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.cubic_to(
            self.x + x1 * self.k,
            self.y - y1 * self.k,
            self.x + x2 * self.k,
            self.y - y2 * self.k,
            self.x + x * self.k,
            self.y - y * self.k,
        );
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// The outlines of `text` with its baseline starting at `(x, y)`, as one path.
pub fn text_path(text: &str, face: Face, size: f32, x: f32, y: f32) -> Path {
    let (glyphs, _) = layout(text, face, size);
    let mut path = Path::new();
    for g in glyphs {
        let mut o = Outline {
            path: &mut path,
            x: x + g.x,
            y,
            k: size / g.font.units_per_em,
        };
        let _ = g.font.face.outline_glyph(g.glyph, &mut o);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_load_and_measure() {
        for face in [Face::Sans, Face::SansMedium, Face::Mono] {
            assert!(face.font().is_some(), "{face:?}");
        }
        let w = text_width("Hello", Face::Sans, 20.0);
        assert!(w > 40.0 && w < 60.0, "{w}");
        // Mono digits are all one width
        assert!(
            (text_width("1111", Face::Mono, 10.0) - text_width("8888", Face::Mono, 10.0)).abs()
                < 1e-3
        );
        // kerning tightens "AV"
        let av = text_width("AV", Face::Sans, 100.0);
        let sum = text_width("A", Face::Sans, 100.0) + text_width("V", Face::Sans, 100.0);
        assert!(av < sum, "{av} vs {sum}");
        // Greek falls back to Sans in Mono; µ is in both
        assert!(text_width("λ", Face::Mono, 10.0) > 0.0);
        let m = metrics(Face::Sans, 10.0);
        assert!(m.cap_height > 6.0 && m.cap_height < 7.5, "{m:?}");
        assert!(!text_path("µm", Face::Sans, 12.0, 0.0, 12.0).is_empty());
    }

    #[test]
    fn bad_woff_is_rejected() {
        assert!(woff_to_sfnt(b"nope").is_none());
        let mut w = SANS_WOFF.to_vec();
        w.truncate(100);
        assert!(woff_to_sfnt(&w).is_none());
    }
}
