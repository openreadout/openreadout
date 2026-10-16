//! Diffraction geometry shared by the area-detector readers (CBF, SMV, NeXus NXmx): what a
//! reader knows about the beam, the detector and the rotation, stored on an image as
//! `extra.diffraction` in one shape, so that `info`, the experiment summary and the preview
//! (log contrast, masked pixels, resolution rings) treat every diffraction format alike.
//!
//! Lengths are in millimetres and ångström, angles in degrees, the beam centre in pixels as the
//! rulers of a preview count them: `x` along a row (the fast axis), `y` down the rows, from the
//! outer corner of the first pixel.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::ImageInfo;

/// The `ImageInfo::extra` key that holds a [`Geometry`].
pub const EXTRA_KEY: &str = "diffraction";

/// Planck constant × speed of light in eV·Å, for X-ray energy ↔ wavelength.
const HC_EV_A: f64 = 12_398.419_843;

/// What was diffracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Radiation {
    /// X-rays (synchrotron or home source).
    Xray,
    /// Electrons (3D ED / MicroED on a transmission electron microscope).
    Electron,
}

/// One diffraction image's (or sweep's) geometry. Every field is optional: readers fill what
/// the file records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Geometry {
    /// X-rays or electrons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiation: Option<Radiation>,
    /// Wavelength in ångström.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wavelength_a: Option<f64>,
    /// Sample-to-detector distance in millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_mm: Option<f64>,
    /// Where the direct beam meets the detector, `[x, y]` in pixels (x along a row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam_center_px: Option<[f64; 2]>,
    /// Pixel size `[x, y]` in millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size_mm: Option<[f64; 2]>,
    /// Detector swing (2θ arm) in degrees, when recorded and not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub two_theta_deg: Option<f64>,
    /// Name of the rotation axis (`phi`, `omega`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_axis: Option<String>,
    /// Rotation angle at the start of the first image, in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oscillation_start_deg: Option<f64>,
    /// Rotation per image, in degrees (0 for stills).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oscillation_width_deg: Option<f64>,
    /// Exposure time per image in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_s: Option<f64>,
    /// Counts at or above which a pixel is overloaded (the detector's trusted maximum).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overload: Option<f64>,
    /// Pixel values that mean "no measurement" (module gaps, dead pixels): negative values on
    /// Pilatus CBF images, the type's maximum on masked Eiger images.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked_values: Vec<f64>,
    /// True when every value below zero means "no measurement".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub negative_is_masked: bool,
}

impl Geometry {
    /// The geometry stored on `im`, if any.
    pub fn of(im: &ImageInfo) -> Option<Geometry> {
        im.extra
            .get(EXTRA_KEY)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Store this geometry on `im` (`extra.diffraction`).
    pub fn store(&self, im: &mut ImageInfo) {
        if let Ok(v) = serde_json::to_value(self) {
            im.extra.insert(EXTRA_KEY.into(), v);
        }
    }

    /// X-ray photon energy in eV, from the wavelength.
    pub fn energy_ev(&self) -> Option<f64> {
        match (self.radiation, self.wavelength_a) {
            (Some(Radiation::Electron), _) => None,
            (_, Some(w)) if w > 0.0 => Some(HC_EV_A / w),
            _ => None,
        }
    }

    /// True when `v` is a "no measurement" value.
    pub fn is_masked(&self, v: f64) -> bool {
        (self.negative_is_masked && v < 0.0) || self.masked_values.contains(&v)
    }

    /// True when `v` is overloaded.
    pub fn is_overloaded(&self, v: f64) -> bool {
        self.overload.is_some_and(|o| v >= o)
    }

    /// Radius in pixels of the ring of `d_a` ångström resolution, for a detector normal to the
    /// beam (2θ = 0), and the geometry it needs: wavelength, distance and a square-ish pixel.
    pub fn ring_radius_px(&self, d_a: f64) -> Option<f64> {
        let (w, dist, px) = self.flat_geometry()?;
        let s = w / (2.0 * d_a);
        if !(0.0..1.0).contains(&s) {
            return None;
        }
        let two_theta = 2.0 * s.asin();
        if two_theta >= std::f64::consts::FRAC_PI_2 {
            return None;
        }
        Some(dist * two_theta.tan() / px)
    }

    /// Resolution in ångström at `r_px` pixels from the beam centre (flat detector, 2θ = 0).
    pub fn resolution_at_radius(&self, r_px: f64) -> Option<f64> {
        let (w, dist, px) = self.flat_geometry()?;
        if r_px <= 0.0 {
            return None;
        }
        let two_theta = (r_px * px / dist).atan();
        let s = (two_theta / 2.0).sin();
        (s > 0.0).then(|| w / (2.0 * s))
    }

    /// (wavelength Å, distance mm, pixel size mm) when rings can be drawn: a detector normal to
    /// the beam, a known beam centre and pixel size.
    fn flat_geometry(&self) -> Option<(f64, f64, f64)> {
        let w = self.wavelength_a.filter(|w| *w > 0.0 && w.is_finite())?;
        let dist = self.distance_mm.filter(|d| *d > 0.0 && d.is_finite())?;
        let [px, py] = self.pixel_size_mm?;
        if !(px > 0.0 && py > 0.0) || (px / py - 1.0).abs() > 0.01 {
            return None;
        }
        self.beam_center_px?;
        if self.two_theta_deg.is_some_and(|t| t.abs() > 0.01) {
            return None;
        }
        Some((w, dist, px))
    }

    /// The best resolution in ångström anywhere on a `width` × `height` detector (its farthest
    /// corner from the beam centre).
    pub fn corner_resolution(&self, width: u32, height: u32) -> Option<f64> {
        let [bx, by] = self.beam_center_px?;
        let corners = [
            (0.0, 0.0),
            (f64::from(width), 0.0),
            (0.0, f64::from(height)),
            (f64::from(width), f64::from(height)),
        ];
        let r = corners
            .iter()
            .map(|(x, y)| ((x - bx).powi(2) + (y - by).powi(2)).sqrt())
            .fold(0.0, f64::max);
        self.resolution_at_radius(r)
    }

    /// The resolution in ångström at the middle of the nearest edge (what a circular
    /// resolution limit inside the detector reaches).
    pub fn edge_resolution(&self, width: u32, height: u32) -> Option<f64> {
        let [bx, by] = self.beam_center_px?;
        let r = bx
            .min(by)
            .min(f64::from(width) - bx)
            .min(f64::from(height) - by);
        self.resolution_at_radius(r)
    }
}

/// The experiment of a file of diffraction images: the technique (single-crystal X-ray or
/// electron diffraction), the beam and detector settings of the first image as method
/// parameters, and one measurement per image. `None` when no image has a geometry.
pub fn experiment(images: &[ImageInfo]) -> Option<crate::experiment::Experiment> {
    use std::fmt::Write as _;
    let (first, g) = images.iter().find_map(|im| Some((im, Geometry::of(im)?)))?;
    let from = |field: &str| format!("images[{}].extra.diffraction.{field}", first.index);
    let mut f = crate::series::Facts::new();
    let electron = g.radiation == Some(Radiation::Electron);
    let (technique, kind) = if electron {
        ("CHMO:0000142", "OBI:0000990")
    } else {
        ("CHMO:0000159", "CHMO:0002105")
    };
    f.instrument_kind(kind)
        .technique(technique, &from("radiation"));
    if let Some(w) = g.wavelength_a {
        f.number("wavelength", w, "Å", &from("wavelength_a"));
    }
    if let Some(e) = g.energy_ev() {
        f.number("energy", (e * 10.0).round() / 10.0, "eV", &from("wavelength_a"));
    }
    if let Some(d) = g.distance_mm {
        f.number("detector_distance", d, "mm", &from("distance_mm"));
    }
    if let Some([x, y]) = g.beam_center_px {
        f.plain("beam_center_px", serde_json::json!([x, y]), &from("beam_center_px"));
    }
    if let Some([px, _]) = g.pixel_size_mm {
        f.number("detector_pixel_size", px, "mm", &from("pixel_size_mm"));
    }
    if let Some(a) = &g.scan_axis {
        f.plain("rotation_axis", a.clone(), &from("scan_axis"));
    }
    if let Some(s) = g.oscillation_start_deg {
        f.number("oscillation_start", s, "°", &from("oscillation_start_deg"));
    }
    if let Some(w) = g.oscillation_width_deg {
        f.number("oscillation_width", w, "°", &from("oscillation_width_deg"));
    }
    if let Some(t) = g.exposure_s {
        f.number("exposure_time", t, "s", &from("exposure_s"));
    }
    if let Some(t) = g.two_theta_deg {
        f.number("two_theta", t, "°", &from("two_theta_deg"));
    }
    for im in images {
        let g = Geometry::of(im).unwrap_or_default();
        let mut what = String::from(if electron {
            "electron diffraction"
        } else {
            "X-ray diffraction"
        });
        let n = im.size_t.max(im.size_z);
        if n > 1 {
            let _ = write!(what, ", {n} images");
        } else {
            what.push_str(" image");
        }
        let _ = write!(what, ", {} × {} px", im.size_x, im.size_y);
        if let Some(w) = g.wavelength_a {
            let _ = write!(what, ", λ {} Å", short(w));
        }
        if let Some(d) = g.distance_mm {
            let _ = write!(what, ", {} mm", (d * 100.0).round() / 100.0);
        }
        if let Some(w) = g.oscillation_width_deg.filter(|w| *w > 0.0) {
            let _ = write!(what, ", {}° per image", short(w));
        }
        if let Some(r) = g.corner_resolution(im.size_x, im.size_y) {
            let _ = write!(what, ", {:.2} Å in the corners", r);
        }
        f.measurement(
            crate::experiment::MeasurementKind::Image,
            vec![im.index],
            what,
            Some(technique),
        );
    }
    Some(f.build())
}

/// `v` with at most 4 significant decimals, trailing zeros dropped (0.9795, 0.15, 0.02501).
fn short(v: f64) -> String {
    let digits = if v.abs() >= 1.0 { 4 } else { (4.0 - v.abs().log10().floor()).clamp(4.0, 10.0) as usize };
    let s = format!("{v:.digits$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// Electron wavelength in ångström for an accelerating voltage in volts (relativistic).
pub fn electron_wavelength_a(volts: f64) -> Option<f64> {
    if !(volts > 0.0 && volts.is_finite()) {
        return None;
    }
    // λ = h / sqrt(2 m e V (1 + e V / (2 m c²)))
    const H: f64 = 6.626_070_15e-34;
    const M: f64 = 9.109_383_701_5e-31;
    const E: f64 = 1.602_176_634e-19;
    const C: f64 = 299_792_458.0;
    let p = (2.0 * M * E * volts * (1.0 + E * volts / (2.0 * M * C * C))).sqrt();
    Some(H / p * 1e10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pilatus() -> Geometry {
        Geometry {
            radiation: Some(Radiation::Xray),
            wavelength_a: Some(0.9795),
            distance_mm: Some(190.18),
            beam_center_px: Some([1235.34, 1279.08]),
            pixel_size_mm: Some([0.172, 0.172]),
            ..Geometry::default()
        }
    }

    #[test]
    fn rings_and_resolution_invert_each_other() {
        let g = pilatus();
        for d in [10.0, 3.0, 2.0, 1.5] {
            let r = g.ring_radius_px(d).unwrap();
            let back = g.resolution_at_radius(r).unwrap();
            assert!((back - d).abs() < 1e-9, "{d} → {r} → {back}");
        }
        // 2 Å at 0.9795 Å and 190 mm: 2θ = 28.35°, r = 190.18 tan(28.35°) / 0.172
        let r = g.ring_radius_px(2.0).unwrap();
        assert!((r - 596.7).abs() < 0.5, "{r}");
        assert!(g.ring_radius_px(0.4).is_none()); // beyond 2θ = 90°
        let corner = g.corner_resolution(2463, 2527).unwrap();
        assert!(corner > 1.0 && corner < 1.6, "{corner}");
    }

    #[test]
    fn x_ray_energy_and_electron_wavelength() {
        let g = pilatus();
        assert!((g.energy_ev().unwrap() - 12_658.0).abs() < 1.0);
        // 200 kV: 0.02508 Å; 300 kV: 0.01969 Å
        assert!((electron_wavelength_a(200e3).unwrap() - 0.025_08).abs() < 1e-5);
        assert!((electron_wavelength_a(300e3).unwrap() - 0.019_69).abs() < 1e-5);
    }

    #[test]
    fn masks_and_round_trip() {
        let mut g = pilatus();
        g.negative_is_masked = true;
        g.overload = Some(495_976.0);
        assert!(g.is_masked(-1.0) && g.is_masked(-2.0) && !g.is_masked(0.0));
        assert!(g.is_overloaded(495_976.0) && !g.is_overloaded(1.0));
        let mut im = ImageInfo::new(0, 2463, 2527, crate::PixelType::Int32);
        g.store(&mut im);
        assert_eq!(Geometry::of(&im), Some(g));
        let e = experiment(&[im]).unwrap();
        let m = e.method.unwrap();
        assert_eq!(m.technique.unwrap().id, "CHMO:0000159");
        assert!(m.parameters.contains_key("wavelength") && m.parameters.contains_key("energy"));
        assert!(e.measurements[0].what.starts_with("X-ray diffraction image, 2463 × 2527 px"));
    }
}
