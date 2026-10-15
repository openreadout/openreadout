//! The OpenReadout look: paper and ink, a warm red accent, quiet axes, direct labels.
//!
//! A [`Theme`] holds the colours and the type sizes of a figure. Sizes follow the figure's
//! longer side ([`Theme::for_size`]), so a 768 px picture and a 2048 px one read the same when
//! each is shown at its natural size.

use crate::color::{Color, Colormap};

/// Light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Ink on paper (default).
    #[default]
    Light,
    /// Light ink on a dark ground.
    Dark,
}

/// Colours and type sizes of a figure.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Light or dark.
    pub mode: Mode,
    /// Figure background.
    pub paper: Color,
    /// Background of a plot panel or small-multiple card (slightly off the paper).
    pub panel: Color,
    /// Strongest text and the main data colour.
    pub ink: Color,
    /// Secondary text: tick labels, captions.
    pub muted: Color,
    /// Axis lines, card outlines, rules.
    pub rule: Color,
    /// Grid lines (fainter than rules).
    pub grid: Color,
    /// The accent: what the figure is about (a highlighted series, a marked peak).
    pub accent: Color,
    /// A soft fill in the accent's family, for bands and highlighted ranges.
    pub band: Color,
    /// Data that recedes: context series, blank wells.
    pub faint: Color,
    /// Categorical series colours, in order (ink and accent first).
    pub series: Vec<Color>,
    /// Sequential colour map for heat maps (low to high).
    pub sequential: Colormap,
    /// Diverging colour map (low, middle, high).
    pub diverging: Colormap,
    /// Base type size in pixels (tick labels and direct labels).
    pub font_size: f32,
    /// Main line width in pixels.
    pub line_width: f32,
}

impl Theme {
    /// The light theme at a 13 px base size.
    pub fn light() -> Theme {
        Theme {
            mode: Mode::Light,
            paper: Color::hex(0xF7F5F0),
            panel: Color::hex(0xFBFAF7),
            ink: Color::hex(0x1D1F24),
            muted: Color::hex(0x6B6E75),
            rule: Color::hex(0xD6D1C7),
            grid: Color::hex(0xE9E5DD),
            accent: Color::hex(0xA8432F),
            band: Color::hex(0xEFE7DF),
            faint: Color::hex(0xCFCAC0),
            series: vec![
                Color::hex(0x1D1F24),
                Color::hex(0xA8432F),
                Color::hex(0x3D6A8A),
                Color::hex(0xC08A2E),
                Color::hex(0x5E7F4F),
                Color::hex(0x7B5B84),
                Color::hex(0x2E7D7A),
                Color::hex(0x8C6D5A),
            ],
            sequential: Colormap::new(vec![
                Color::hex(0xEDE2CC),
                Color::hex(0xE9D2A6),
                Color::hex(0xE3B57F),
                Color::hex(0xD18A57),
                Color::hex(0xB45C3C),
                Color::hex(0x8A3A2C),
                Color::hex(0x55222A),
            ]),
            diverging: Colormap::new(vec![
                Color::hex(0x2F5A78),
                Color::hex(0x8FB0C4),
                Color::hex(0xF3EEE4),
                Color::hex(0xD9A08A),
                Color::hex(0xA8432F),
            ]),
            font_size: 13.0,
            line_width: 1.5,
        }
    }

    /// The dark theme at a 13 px base size.
    pub fn dark() -> Theme {
        Theme {
            mode: Mode::Dark,
            paper: Color::hex(0x16171A),
            panel: Color::hex(0x1C1D21),
            ink: Color::hex(0xECEAE4),
            muted: Color::hex(0x9A9CA3),
            rule: Color::hex(0x3A3C42),
            grid: Color::hex(0x2A2C31),
            accent: Color::hex(0xE07A5F),
            band: Color::hex(0x3A2A26),
            faint: Color::hex(0x4A4C52),
            series: vec![
                Color::hex(0xECEAE4),
                Color::hex(0xE07A5F),
                Color::hex(0x7FA8C6),
                Color::hex(0xE0B25A),
                Color::hex(0x92B57F),
                Color::hex(0xB497BD),
                Color::hex(0x5FB3AD),
                Color::hex(0xC0A08A),
            ],
            sequential: Colormap::new(vec![
                Color::hex(0x22202A),
                Color::hex(0x4A2531),
                Color::hex(0x7A3333),
                Color::hex(0xA84E37),
                Color::hex(0xCF7A4A),
                Color::hex(0xE7AE6E),
                Color::hex(0xF6E2B4),
            ]),
            diverging: Colormap::new(vec![
                Color::hex(0x7FA8C6),
                Color::hex(0x3E5566),
                Color::hex(0x26262B),
                Color::hex(0x7A3F33),
                Color::hex(0xE07A5F),
            ]),
            font_size: 13.0,
            line_width: 1.5,
        }
    }

    /// The theme for `mode`.
    pub fn new(mode: Mode) -> Theme {
        match mode {
            Mode::Light => Theme::light(),
            Mode::Dark => Theme::dark(),
        }
    }

    /// Type and line sizes for a figure whose longer side is `long_side` pixels: 12 px text at
    /// 768 px, 16 px at 1024 px, at most 24 px; never under 9 px.
    pub fn for_size(mut self, long_side: f32) -> Theme {
        let s = (long_side / 64.0).clamp(9.0, 24.0);
        self.font_size = s;
        self.line_width = (s / 8.5).clamp(1.0, 2.6);
        self
    }

    /// Series colour `i` (cycling).
    pub fn series(&self, i: usize) -> Color {
        if self.series.is_empty() {
            return self.ink;
        }
        self.series[i % self.series.len()]
    }

    /// Type size for titles.
    pub fn title_size(&self) -> f32 {
        (self.font_size * 1.25).round()
    }

    /// Type size for small print (units, captions under a scale bar).
    pub fn small_size(&self) -> f32 {
        (self.font_size * 0.85).max(8.0)
    }

    /// Hairline width for axes, ticks and rules.
    pub fn hairline(&self) -> f32 {
        (self.line_width * 0.67).max(1.0)
    }

    /// Text colour that reads on `bg`: ink-dark on light grounds, paper-light on dark ones.
    pub fn text_on(&self, bg: Color) -> Color {
        if bg.luminance() > 0.35 {
            Color::hex(0x1D1F24)
        } else {
            Color::hex(0xF7F5F0)
        }
    }
}

impl Theme {
    /// The data colours of this theme as a JSON object (`paper`, `panel`, `ink`, `muted`,
    /// `rule`, `grid`, `accent`, `band`, `faint` as `#RRGGBB`; `series`, `sequential` and
    /// `diverging` as arrays of them).
    pub fn tokens(&self) -> String {
        let one = |c: Color| format!("\"{}\"", c.to_hex());
        let many = |cs: &[Color]| {
            let v: Vec<String> = cs.iter().map(|c| one(*c)).collect();
            format!("[{}]", v.join(","))
        };
        let named = [
            ("paper", self.paper),
            ("panel", self.panel),
            ("ink", self.ink),
            ("muted", self.muted),
            ("rule", self.rule),
            ("grid", self.grid),
            ("accent", self.accent),
            ("band", self.band),
            ("faint", self.faint),
        ];
        let mut parts: Vec<String> = named
            .iter()
            .map(|(k, c)| format!("\"{k}\":{}", one(*c)))
            .collect();
        parts.push(format!("\"series\":{}", many(&self.series)));
        parts.push(format!("\"sequential\":{}", many(self.sequential.stops())));
        parts.push(format!("\"diverging\":{}", many(self.diverging.stops())));
        format!("{{{}}}", parts.join(","))
    }

    /// Both themes' tokens, `{"light": {...}, "dark": {...}}`: what the viewer draws data with,
    /// so its colours stay those of the pictures.
    pub fn tokens_json() -> String {
        format!(
            "{{\"light\":{},\"dark\":{}}}",
            Theme::light().tokens(),
            Theme::dark().tokens()
        )
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_figure() {
        assert!((Theme::light().for_size(768.0).font_size - 12.0).abs() < 1e-6);
        assert!((Theme::light().for_size(1024.0).font_size - 16.0).abs() < 1e-6);
        assert!((Theme::light().for_size(100.0).font_size - 9.0).abs() < 1e-6);
        let t = Theme::dark();
        assert_eq!(t.series(9), t.series[1]);
        assert_eq!(t.text_on(Color::rgb(0, 0, 0)), Color::hex(0xF7F5F0));
    }
}
