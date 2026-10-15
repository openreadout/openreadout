//! A preview request as JSON, with the names `openreadout_preview` uses. The Python, R and
//! WebAssembly bindings take their preview options in this form, so the three agree with the
//! MCP tool on every name and default.

use openreadout_core::{Error, Region, Result};
use serde::{Deserialize, Deserializer};

use crate::{Annotation, Axes, Axis, Contrast, Lut, Marks, PreviewRequest};

/// One string or a list of them.
fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::One(s)) => vec![s],
        Some(OneOrMany::Many(v)) => v,
    })
}

/// Preview options by name. Every field is optional; [`PreviewOptions::request`] fills in the
/// defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PreviewOptions {
    /// Image index.
    pub image: Option<u32>,
    /// Plane selection (`c=1`, `z=4`, or combined `c=0,1,z=2`).
    #[serde(default, deserialize_with = "one_or_many")]
    pub select: Vec<String>,
    /// Maximum-intensity projection axis.
    pub mip: Option<Axis>,
    /// Blend the channels in their colours.
    #[serde(default)]
    pub composite: bool,
    /// Pyramid level.
    pub level: Option<u32>,
    /// Only this rectangle, `{x, y, width, height}`.
    pub region: Option<Region>,
    /// Longest side in pixels.
    pub max_size: Option<u32>,
    /// What frames an image (`rulers`, `grid`, `none`).
    pub axes: Option<Axes>,
    /// `auto`, `min-max`, `percentile:LO,HI` or `raw`.
    pub contrast: Option<String>,
    /// Colour lookup of one channel.
    pub lut: Option<Lut>,
    /// Trace index.
    pub trace: Option<u32>,
    /// Sweep index.
    pub sweep: Option<u32>,
    /// Trace channel indices.
    #[serde(default)]
    pub channels: Vec<u32>,
    /// Spectrum run index.
    pub run: Option<u32>,
    /// Zero-based spectrum index.
    pub spectrum: Option<u64>,
    /// Instrument scan number.
    pub scan: Option<u64>,
    /// The instrument's centroid list.
    #[serde(default)]
    pub centroid: bool,
    /// Table index (plates, flow cytometry).
    pub table: Option<u32>,
    /// Table columns: a plate's value column, or the parameters of an event plot.
    #[serde(default, deserialize_with = "one_or_many")]
    pub columns: Vec<String>,
    /// Marks from the data.
    pub marks: Option<Marks>,
    /// The caller's own marks.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// Draw in the dark theme.
    #[serde(default)]
    pub dark: bool,
}

impl PreviewOptions {
    /// Parse options from JSON text; empty text gives the defaults.
    pub fn from_json(text: &str) -> Result<PreviewOptions> {
        if text.trim().is_empty() {
            return Ok(PreviewOptions::default());
        }
        serde_json::from_str(text).map_err(|e| Error::Usage(format!("preview options: {e}")))
    }

    /// The request these options describe. `default_size` is the `max_size` when none is given
    /// and `default_axes` the frame of image previews.
    pub fn request(&self, default_size: u32, default_axes: Axes) -> Result<PreviewRequest> {
        let mut r = PreviewRequest {
            image: self.image,
            select: self.select.clone(),
            mip: self.mip,
            composite: self.composite,
            level: self.level,
            region: self.region,
            max_size: self.max_size.unwrap_or(default_size),
            contrast: match self.contrast.as_deref() {
                None => Contrast::Auto,
                Some(s) => s.parse()?,
            },
            lut: self.lut,
            trace: self.trace,
            sweep: self.sweep,
            channels: self.channels.clone(),
            run: self.run,
            spectrum: self.spectrum,
            scan: self.scan,
            centroid: self.centroid,
            table: self.table,
            columns: self.columns.clone(),
            marks: self.marks.unwrap_or_default(),
            annotations: self.annotations.clone(),
            dark: self.dark,
            ..PreviewRequest::default()
        };
        self.axes.unwrap_or(default_axes).apply(&mut r);
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_defaults() {
        let o = PreviewOptions::from_json(
            r#"{"select": "c=1", "columns": ["FSC-A", "SSC-A"], "axes": "grid", "dark": true,
                "annotations": [{"type": "point", "x": 10, "y": 20, "label": "a"}],
                "contrast": "percentile:1,99", "marks": "spikes"}"#,
        )
        .unwrap();
        let r = o.request(512, Axes::None).unwrap();
        assert_eq!(r.select, vec!["c=1"]);
        assert_eq!(r.columns.len(), 2);
        assert!(r.axes && r.grid && r.dark);
        assert_eq!(r.max_size, 512);
        assert_eq!(r.contrast, Contrast::Percentile(1.0, 99.0));
        assert_eq!(r.marks, Marks::Spikes);
        assert_eq!(r.annotations.len(), 1);
        let bare = PreviewOptions::from_json("").unwrap();
        assert!(!bare.request(512, Axes::None).unwrap().axes);
    }

    #[test]
    fn unknown_keys_are_refused() {
        let e = PreviewOptions::from_json(r#"{"colour": 1}"#).unwrap_err();
        assert!(e.to_string().contains("colour"), "{e}");
        assert!(PreviewOptions::from_json(r#"{"contrast": "loud"}"#)
            .unwrap()
            .request(512, Axes::Rulers)
            .is_err());
    }
}
