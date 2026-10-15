//! One rule for colouring categories (plate layout groups, flow populations, plot series), so
//! the same plate looks the same in a preview, a plot and the viewer.
//!
//! Categories take the theme's series colours in the order they first appear (plate reading
//! order, series order): ink, the accent, blue, ochre, green, purple, teal, brown, then round
//! again. Blank-like categories (`blank`, `BL`, `buffer`, `empty`, …) are drawn faint and do not
//! use up a colour. The viewer applies the same rule in JavaScript (`categoryColors` in
//! `viewer.js`), with the same names and colours from [`crate::Theme::tokens_json`].

use crate::color::Color;
use crate::theme::Theme;

/// Labels that mean "no sample here": blank wells, buffer, empty, media.
pub const BLANK_LABELS: [&str; 10] = [
    "blank",
    "bl",
    "blk",
    "buffer",
    "empty",
    "medium",
    "media",
    "background",
    "none",
    "-",
];

/// True when a layout label marks a blank (case and surrounding space ignored).
pub fn is_blank_label(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    BLANK_LABELS.contains(&s.as_str())
}

/// The colour of each category in `names` (in first-appearance order; repeats get the same
/// colour): blanks faint, the rest the theme's series colours in turn.
pub fn category_colors<S: AsRef<str>>(theme: &Theme, names: &[S]) -> Vec<Color> {
    let mut seen: Vec<&str> = Vec::new();
    names
        .iter()
        .map(|n| {
            let n = n.as_ref();
            if is_blank_label(n) {
                return theme.faint;
            }
            let i = match seen.iter().position(|s| *s == n) {
                Some(i) => i,
                None => {
                    seen.push(n);
                    seen.len() - 1
                }
            };
            theme.series(i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_take_series_colours_in_order() {
        let t = Theme::light();
        let c = category_colors(&t, &["lambda", "BL", "HK97", "lambda"]);
        assert_eq!(c, vec![t.series(0), t.faint, t.series(1), t.series(0)]);
        assert!(is_blank_label(" Blank "));
        assert!(!is_blank_label("blanket"));
    }
}
