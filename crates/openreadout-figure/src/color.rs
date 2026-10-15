//! Colours and colour maps.

/// An 8-bit sRGB colour with alpha (255 = opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Opacity.
    pub a: u8,
}

impl Color {
    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// An opaque colour from `0xRRGGBB`.
    pub const fn hex(v: u32) -> Color {
        Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Parse `#RRGGBB`, `#RRGGBBAA` or `#RGB` (the `#` is optional).
    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        if !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(h.get(i..=i)?, 16).ok().map(|v| v * 17);
                Some(Color::rgb(n(0)?, n(1)?, n(2)?))
            }
            6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Color {
                a: byte(6)?,
                ..Color::rgb(byte(0)?, byte(2)?, byte(4)?)
            }),
            _ => None,
        }
    }

    /// `#RRGGBB` (opacity left out).
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// The same colour at opacity `alpha` (0–1).
    pub fn alpha(self, alpha: f32) -> Color {
        Color {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// Opacity as 0–1.
    pub fn opacity(self) -> f32 {
        f32::from(self.a) / 255.0
    }

    /// Mix with `other`: `t` = 0 gives `self`, 1 gives `other` (opacity mixed too).
    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Color {
            r: m(self.r, other.r),
            g: m(self.g, other.g),
            b: m(self.b, other.b),
            a: m(self.a, other.a),
        }
    }

    /// Relative luminance (0 = black, 1 = white), sRGB.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// `[r, g, b]`.
    pub fn to_rgb(self) -> [u8; 3] {
        [self.r, self.g, self.b]
    }

    /// From `[r, g, b]`.
    pub fn from_rgb(c: [u8; 3]) -> Color {
        Color::rgb(c[0], c[1], c[2])
    }
}

/// A colour map: colours at evenly spaced stops from 0 to 1, interpolated linearly.
#[derive(Debug, Clone, PartialEq)]
pub struct Colormap {
    stops: Vec<Color>,
}

impl Colormap {
    /// A map through `stops` (at least one).
    pub fn new(stops: Vec<Color>) -> Colormap {
        let stops = if stops.is_empty() {
            vec![Color::rgb(0, 0, 0)]
        } else {
            stops
        };
        Colormap { stops }
    }

    /// The colour at `t` in `[0, 1]` (clamped; NaN gives the first stop).
    pub fn at(&self, t: f64) -> Color {
        let n = self.stops.len();
        if n == 1 {
            return self.stops[0];
        }
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = t * (n - 1) as f64;
        let i = (x.floor() as usize).min(n - 2);
        self.stops[i].mix(self.stops[i + 1], (x - i as f64) as f32)
    }

    /// The same map, reversed.
    pub fn reversed(&self) -> Colormap {
        let mut s = self.stops.clone();
        s.reverse();
        Colormap { stops: s }
    }

    /// The stops.
    pub fn stops(&self) -> &[Color] {
        &self.stops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_mix() {
        assert_eq!(Color::parse("#a8432f"), Some(Color::hex(0xA8432F)));
        assert_eq!(Color::parse("fff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::parse("#00000080").map(|c| c.a), Some(128));
        assert_eq!(Color::parse("#12"), None);
        assert_eq!(Color::hex(0x0102FF).to_hex(), "#0102FF");
        let m = Color::rgb(0, 0, 0).mix(Color::rgb(255, 255, 255), 0.5);
        assert_eq!(m, Color::rgb(128, 128, 128));
        let cm = Colormap::new(vec![Color::rgb(0, 0, 0), Color::rgb(200, 0, 0)]);
        assert_eq!(cm.at(0.5), Color::rgb(100, 0, 0));
        assert_eq!(cm.at(f64::NAN), Color::rgb(0, 0, 0));
        assert!(Color::rgb(255, 255, 255).luminance() > 0.99);
    }
}
