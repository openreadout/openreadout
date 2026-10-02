//! Colours: channel colours from the normalized model, the documented default palette, a
//! wavelength-to-colour mapping, and the sequential colour map used by plate heat maps.

use openreadout_core::model::ChannelInfo;

/// An 8-bit sRGB colour.
pub type Rgb = [u8; 3];

/// Default channel colours, in order, for channels whose file records neither a display colour
/// nor a wavelength: green, magenta, cyan, red, blue, yellow, orange, white. Green and magenta
/// come first so a two-channel composite stays readable for red-green colour-blind viewers.
pub const DEFAULT_PALETTE: [(&str, Rgb); 8] = [
    ("#00FF00", [0, 255, 0]),
    ("#FF00FF", [255, 0, 255]),
    ("#00FFFF", [0, 255, 255]),
    ("#FF0000", [255, 0, 0]),
    ("#0000FF", [0, 0, 255]),
    ("#FFFF00", [255, 255, 0]),
    ("#FF8000", [255, 128, 0]),
    ("#FFFFFF", [255, 255, 255]),
];

/// Line colours for trace sparklines (dark enough to read on white).
pub const TRACE_PALETTE: [Rgb; 8] = [
    [31, 119, 180],
    [214, 39, 40],
    [44, 160, 44],
    [148, 103, 189],
    [255, 127, 14],
    [140, 86, 75],
    [227, 119, 194],
    [23, 190, 207],
];

/// Where a channel's preview colour came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorSource {
    /// `channels[].color` recorded in the file.
    File,
    /// Derived from `emission_nm` / `emission_range_nm` / `excitation_nm`.
    Wavelength,
    /// The default palette, by position.
    Palette,
}

impl ColorSource {
    /// The lowercase name used in JSON (`file`, `wavelength`, `palette`).
    pub fn as_str(self) -> &'static str {
        match self {
            ColorSource::File => "file",
            ColorSource::Wavelength => "wavelength",
            ColorSource::Palette => "palette",
        }
    }
}

/// Parse `#RRGGBB` (the normalized model's spelling; the `#` is optional).
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// `#RRGGBB` for a colour.
pub fn to_hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Saturated display colour for a wavelength in nm (380–780 nm; shorter is violet, longer red).
/// A piecewise-linear hue ramp through violet, blue, cyan, green, yellow and red, at full
/// brightness so that a channel's colour never dims its data.
pub fn wavelength_rgb(nm: f64) -> Option<Rgb> {
    if !nm.is_finite() || nm <= 0.0 {
        return None;
    }
    let l = nm.clamp(380.0, 780.0);
    let (r, g, b) = if l < 440.0 {
        ((440.0 - l) / 60.0, 0.0, 1.0)
    } else if l < 490.0 {
        (0.0, (l - 440.0) / 50.0, 1.0)
    } else if l < 510.0 {
        (0.0, 1.0, (510.0 - l) / 20.0)
    } else if l < 580.0 {
        ((l - 510.0) / 70.0, 1.0, 0.0)
    } else if l < 645.0 {
        (1.0, (645.0 - l) / 65.0, 0.0)
    } else {
        (1.0, 0.0, 0.0)
    };
    let q = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    Some([q(r), q(g), q(b)])
}

/// Colour for channel `ch` shown at `position` among the channels of a composite:
/// `channels[].color` when the file records a non-black colour, else a colour derived from the
/// emission (or detection band, or excitation) wavelength, else the default palette.
pub fn channel_color(ch: Option<&ChannelInfo>, position: usize) -> (Rgb, ColorSource) {
    if let Some(ch) = ch {
        if let Some(c) = ch.color.as_deref().and_then(parse_hex)
            && c != [0, 0, 0]
        {
            return (c, ColorSource::File);
        }
        let nm = ch
            .emission_nm
            .or_else(|| ch.emission_range_nm.map(|[a, b]| f64::midpoint(a, b)))
            .or(ch.excitation_nm);
        if let Some(c) = nm.and_then(wavelength_rgb) {
            return (c, ColorSource::Wavelength);
        }
    }
    (
        DEFAULT_PALETTE[position % DEFAULT_PALETTE.len()].1,
        ColorSource::Palette,
    )
}

/// Anchor points of the sequential colour map used for plate heat maps (dark blue → teal →
/// green → yellow), sampled at 0, 1/8, …, 1 and interpolated linearly.
const SEQUENTIAL: [Rgb; 9] = [
    [68, 1, 84],
    [71, 44, 122],
    [59, 81, 139],
    [44, 113, 142],
    [33, 144, 141],
    [39, 173, 129],
    [92, 200, 99],
    [170, 220, 50],
    [253, 231, 37],
];

/// Sequential colour for `t` in `[0, 1]`.
pub fn sequential(t: f64) -> Rgb {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = t * (SEQUENTIAL.len() - 1) as f64;
    let i = (x.floor() as usize).min(SEQUENTIAL.len() - 2);
    let f = x - i as f64;
    let (a, b) = (SEQUENTIAL[i], SEQUENTIAL[i + 1]);
    let mix = |k: usize| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f).round() as u8;
    [mix(0), mix(1), mix(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("00ff00"), Some([0, 255, 0]));
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(to_hex([1, 2, 255]), "#0102FF");
    }

    #[test]
    fn wavelengths_map_to_expected_hues() {
        assert_eq!(wavelength_rgb(460.0), Some([0, 102, 255])); // DAPI-ish blue
        assert_eq!(wavelength_rgb(520.0).map(|c| c[1]), Some(255)); // GFP green
        assert_eq!(wavelength_rgb(700.0), Some([255, 0, 0]));
        assert_eq!(wavelength_rgb(0.0), None);
    }

    #[test]
    fn channel_color_priority() {
        let mut ch = ChannelInfo {
            color: Some("#000000".into()),
            emission_nm: Some(700.0),
            ..ChannelInfo::default()
        };
        assert_eq!(channel_color(Some(&ch), 0).1, ColorSource::Wavelength);
        ch.color = Some("#123456".into());
        assert_eq!(
            channel_color(Some(&ch), 0),
            ([0x12, 0x34, 0x56], ColorSource::File)
        );
        assert_eq!(
            channel_color(None, 1),
            ([255, 0, 255], ColorSource::Palette)
        );
    }

    #[test]
    fn sequential_endpoints() {
        assert_eq!(sequential(0.0), SEQUENTIAL[0]);
        assert_eq!(sequential(1.0), SEQUENTIAL[8]);
        assert_eq!(sequential(f64::NAN), SEQUENTIAL[0]);
    }
}
