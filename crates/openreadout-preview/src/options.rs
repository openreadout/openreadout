//! A preview request as JSON, with the names `openreadout_preview` uses. The Python, R and
//! WebAssembly bindings take their preview options in this form, so the three agree with the
//! MCP tool on every name and default.

use openreadout_core::{Error, Region, Result};
use schemars::JsonSchema;
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
/// defaults. The MCP tool `openreadout_preview` takes these fields as its arguments, so their
/// documentation is also what an agent reads.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PreviewOptions {
    /// Image index (default 0).
    pub image: Option<u32>,
    /// Plane selection such as `c=1`, `z=4`, `t=0` or combined `c=0,1,z=2`. Default: c=0, the
    /// middle z, t=0. Several channels imply a composite.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Vec<String>")]
    pub select: Vec<String>,
    /// Maximum-intensity projection, over the selected range if any.
    pub mip: Option<Axis>,
    /// Volumes: the plane to show. `axial`, `coronal`, `sagittal` (radiological orientation),
    /// `orthogonal` (all three, crossing at the middle or the selected z), `xz`, `yz`, or
    /// `native` (as stored). Default: medical images in their acquisition plane, others native.
    /// With a view, `z` and `mip: z` count slices of that view.
    pub view: Option<crate::View>,
    /// Blend all (or the selected) channels additively in their colours.
    #[serde(default)]
    pub composite: bool,
    /// Pyramid level to read (default: the level nearest max_size, or the level at which
    /// `region` renders at about max_size).
    pub level: Option<u32>,
    /// Zoom: only this rectangle, `{x, y, width, height}` in full-resolution pixels (or in the
    /// pixels of `level` when a level is given). Whole-slide images: look at the overview
    /// first, then at regions of it.
    pub region: Option<Region>,
    /// Longest side in pixels, rulers included (MCP: default 768, at most 2048).
    pub max_size: Option<u32>,
    /// Image previews: what frames the picture.
    pub axes: Option<Axes>,
    /// Image previews with axes scale-bar: a title above the picture.
    pub title: Option<String>,
    /// Image previews with axes scale-bar: a caption under the picture (what it shows, the
    /// stain, the sample).
    pub caption: Option<String>,
    /// `auto` (default; the file's display window when it has one), `min-max`,
    /// `percentile:LO,HI` (e.g. `percentile:1,99`), `raw`, or a display window
    /// `window:CENTER,WIDTH` (`window:40,400`), `window:file`, or a CT preset `window:soft-tissue`,
    /// `brain`, `subdural`, `stroke`, `bone`, `lung`, `mediastinum`, `liver`.
    pub contrast: Option<String>,
    /// Colour lookup. Default: gray for one channel, channel colours for composites.
    pub lut: Option<Lut>,
    /// Trace preview: trace index (see openreadout_info traces[]).
    pub trace: Option<u32>,
    /// Trace preview: sweep index.
    pub sweep: Option<u32>,
    /// Trace preview: channel indices (default: the first 8).
    #[serde(default)]
    pub channels: Vec<u32>,
    /// Trace preview: only this window of the x axis, `[a, b]` in its units (bp of a sized electropherogram, base positions of a Sanger read, minutes, ppm, …).
    pub x_range: Option<[f64; 2]>,
    /// Spectrum preview: run index.
    pub run: Option<u32>,
    /// Spectrum preview: zero-based spectrum index.
    pub spectrum: Option<u64>,
    /// Spectrum preview: instrument scan number instead of an index.
    pub scan: Option<u64>,
    /// Spectrum preview: the instrument's centroid list instead of the profile.
    #[serde(default)]
    pub centroid: bool,
    /// Plate or flow-cytometry preview: table index.
    pub table: Option<u32>,
    /// Table columns: a plate's value column, or the x and y parameters of a flow-cytometry
    /// density plot (one gives a histogram; default FSC-A, SSC-A).
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Vec<String>")]
    pub columns: Vec<String>,
    /// Marks from the data. Default auto: the prominent peaks of spectra and chromatograms are
    /// labelled.
    pub marks: Option<Marks>,
    /// Images: how objects are found for `marks: objects` (as `openreadout_stats` objects).
    pub objects: Option<openreadout_core::measure::ObjectsRequest>,
    /// Gels and blots: how lanes are found for `marks: lanes` (as `openreadout_stats` lanes).
    pub lanes: Option<openreadout_core::measure::LanesRequest>,
    /// Your own marks, drawn on top and reported back with their pixel positions. Coordinates
    /// as the picture shows them: full-resolution pixels on images (the ruler numbers), axis
    /// units on plots, wells on plates.
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
            view: self.view,
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
            x_range: self.x_range,
            run: self.run,
            spectrum: self.spectrum,
            scan: self.scan,
            centroid: self.centroid,
            table: self.table,
            columns: self.columns.clone(),
            marks: self.marks.unwrap_or_default(),
            objects: self.objects.clone(),
            lanes: self.lanes.clone(),
            annotations: self.annotations.clone(),
            dark: self.dark,
            title: self.title.clone(),
            caption: self.caption.clone(),
            ..PreviewRequest::default()
        };
        // a title or caption asks for the figure frame, which draws them
        let text = self.title.is_some() || self.caption.is_some();
        let axes = self
            .axes
            .unwrap_or(if text { Axes::ScaleBar } else { default_axes });
        axes.apply(&mut r);
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
    fn a_title_asks_for_the_figure_frame() {
        let o = PreviewOptions::from_json(r#"{"title": "Cerebellum"}"#).unwrap();
        let r = o.request(512, Axes::Rulers).unwrap();
        assert!(r.axes && r.figure && !r.grid);
        assert_eq!(r.title.as_deref(), Some("Cerebellum"));
        let o = PreviewOptions::from_json(r#"{"axes": "scale-bar"}"#).unwrap();
        assert!(o.request(512, Axes::Rulers).unwrap().figure);
        let o = PreviewOptions::from_json(r#"{"title": "x", "axes": "rulers"}"#).unwrap();
        assert!(!o.request(512, Axes::None).unwrap().figure);
        assert!(
            !PreviewOptions::default()
                .request(512, Axes::Rulers)
                .unwrap()
                .figure
        );
    }

    #[test]
    fn unknown_keys_are_refused() {
        let e = PreviewOptions::from_json(r#"{"colour": 1}"#).unwrap_err();
        assert!(e.to_string().contains("colour"), "{e}");
        assert!(
            PreviewOptions::from_json(r#"{"contrast": "loud"}"#)
                .unwrap()
                .request(512, Axes::Rulers)
                .is_err()
        );
    }
}
