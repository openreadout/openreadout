//! Counting objects: nuclei, cells or spots, with their sizes, intensities in every channel
//! and positions. The steps and their sources are in `segment.rs`; the definitions users see
//! are in `book/src/reference/commands/stats.md`, "Objects".

// x, y, w, h, c, k are the natural names in image arithmetic.
#![allow(clippy::many_single_char_names)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::segment as sg;
use super::{Grid, Spread, Target, inside, read, spread, usage};
use crate::error::{Error, Result};
use crate::region::Region;

/// Objects listed by default.
pub const DEFAULT_MAX_OBJECTS: usize = 100;
/// Regions up to this many pixels (2896 × 2896) are counted in one piece.
const ONE_PIECE: u64 = 1 << 23;
/// Larger regions are counted in tiles this many pixels a side.
const TILE: u32 = 2048;
/// Most pixels an object count covers (an 8192 × 8192 region).
pub const MAX_OBJECT_PIXELS: u64 = 1 << 26;

/// What to count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// nuclei (default): touching nuclei are split
    #[default]
    Nuclei,
    /// whole cells stained throughout (larger; touching cells are split)
    Cells,
    /// small bright spots (puncta, foci, single molecules), found by a Laplacian of Gaussian
    Spots,
}

/// Bright or dark objects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Polarity {
    /// from the image: dark objects on a bright background (brightfield) or bright ones
    #[default]
    Auto,
    /// objects brighter than the background (fluorescence)
    Bright,
    /// objects darker than the background (brightfield)
    Dark,
}

/// How the threshold is chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThresholdMethod {
    /// Otsu's, or the triangle method when objects are sparse
    #[default]
    Auto,
    /// Otsu's (two classes of similar spread)
    Otsu,
    /// the triangle method (a big background peak, few objects)
    Triangle,
    /// each pixel against its neighbourhood (uneven staining)
    Local,
}

/// What an RGB image's objects are found in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Stain {
    /// hematoxylin for brightfield (stained tissue), the brightest component for fluorescence
    #[default]
    Auto,
    /// the nuclear stain of H&E and IHC, by colour deconvolution
    Hematoxylin,
    /// eosin, by colour deconvolution
    Eosin,
    /// the brown IHC chromogen, by colour deconvolution
    Dab,
    /// one component
    Red,
    /// one component
    Green,
    /// one component
    Blue,
    /// the mean of the components
    Luminance,
}

/// How objects are found. Everything is optional; the defaults suit nuclei.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectsRequest {
    /// Default nuclei.
    #[serde(default)]
    pub kind: Kind,
    /// Expected object diameter in µm. Default: measured from the image (spots: 0.5 µm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter_um: Option<f64>,
    /// The same in pixels, for images without a pixel size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter_px: Option<f64>,
    /// Threshold in raw units of what is measured (spots: the height above the local
    /// background). Default: chosen by `method`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Default auto.
    #[serde(default)]
    pub method: ThresholdMethod,
    /// Default auto.
    #[serde(default)]
    pub polarity: Polarity,
    /// RGB images. Default auto.
    #[serde(default)]
    pub stain: Stain,
    /// Cut touching objects apart (seeded watershed). Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<bool>,
    /// Smallest object kept in µm². Default: a fifth of the expected area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_area_um2: Option<f64>,
    /// Largest object kept in µm². Default: no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_area_um2: Option<f64>,
    /// Objects listed (in reading order). Default 100; the counts cover all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_objects: Option<usize>,
    /// Trace each object's outline (for pictures; not part of the JSON request).
    #[serde(skip)]
    #[schemars(skip)]
    pub outlines: bool,
}

/// One object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Object {
    /// Number, in reading order (the label on a picture of the objects).
    pub id: u32,
    /// Centroid column, in pixels of the level read.
    pub x: f64,
    /// Centroid row.
    pub y: f64,
    /// Pixels.
    pub area_px: u64,
    /// Area in µm².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_um2: Option<f64>,
    /// Diameter of a disc of the same area, in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diameter_um: Option<f64>,
    /// Area over the area of its convex hull (1 = convex; low values suggest merged objects).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solidity: Option<f64>,
    /// Spots: height above the local background in what was measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Mean raw value in each channel of `channels`, in that order.
    pub mean: Vec<f64>,
    /// True when the object touches the edge of the area searched (it may be cut).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edge: bool,
    /// Outline vertices (pixel corners, pixels of the level read), only for pictures.
    #[serde(skip)]
    #[schemars(skip)]
    pub outline: Vec<[f64; 2]>,
}

/// Objects above a threshold in one channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Positive {
    /// Threshold on the object mean.
    pub threshold: f64,
    /// `otsu` (the object means fall in two groups), `background` (the channel's background
    /// plus five robust standard deviations) or `none` (no object is clearly brighter).
    pub method: String,
    /// Objects whose mean is above it.
    pub count: usize,
    /// The same as a fraction of all objects.
    pub fraction: f64,
}

/// One channel measured in every object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObjectChannel {
    /// Channel index (`c`), or the RGB component or stain index.
    pub c: u32,
    /// Channel, component or stain name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Median value outside the objects.
    pub background: f64,
    /// Object means.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<Spread>,
    /// Objects clearly brighter than the rest (or than the background).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positive: Option<Positive>,
}

/// Objects found in one plane.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Objects {
    /// What was counted.
    pub kind: Kind,
    /// What objects were found in: a channel (`c=0 DAPI`), an RGB component, `luminance` or a
    /// stain (`hematoxylin`, `eosin`, `dab`).
    pub measured: String,
    /// The rectangle searched, in pixels of the level read.
    pub region: Region,
    /// Area searched (the region, or the polygon) in µm².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched_um2: Option<f64>,
    /// Object diameter expected, in pixels: it sets the smoothing, the background scale, how
    /// close two objects may be and the smallest object kept.
    pub expected_diameter_px: f64,
    /// The same in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_diameter_um: Option<f64>,
    /// `given`, `measured` (from the image) or `default`.
    pub expected_diameter_from: String,
    /// `bright` or `dark`.
    pub polarity: Polarity,
    /// Threshold, in units of what was measured after background subtraction (spots: height
    /// above the local background).
    pub threshold: f64,
    /// `otsu`, `triangle`, `local`, `noise` (spots: five robust standard deviations of the
    /// background) or `given`.
    pub threshold_method: String,
    /// Whether touching objects were cut apart.
    pub split: bool,
    /// Objects found, including those touching the edge.
    pub count: usize,
    /// Objects touching the edge of the area searched.
    pub touching_edge: usize,
    /// Objects per mm² (edge objects count half).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density_per_mm2: Option<f64>,
    /// Fraction of the area searched covered by objects.
    pub area_fraction: f64,
    /// Objects more than twice the median area: possibly several objects together.
    pub large: usize,
    /// Object areas in pixels (objects off the edge, or all when every one touches it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_px: Option<Spread>,
    /// Object areas in µm².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_um2: Option<Spread>,
    /// Diameters (of a disc of the same area) in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diameter_um: Option<Spread>,
    /// Every channel measured in the objects, in the order of each object's `mean`.
    pub channels: Vec<ObjectChannel>,
    /// The objects in reading order (top to bottom, then left to right).
    pub objects: Vec<Object>,
    /// True when `objects` lists fewer than `count`.
    pub truncated: bool,
}

/// What objects are found in, decided once for the whole area searched.
#[derive(Debug, Clone)]
pub(crate) struct Signal {
    /// `None`: a channel as stored; `Some`: computed from RGB.
    pub stain: Option<Stain>,
    /// Brightfield RGB: the light through empty glass, per component.
    pub i0: [f32; 3],
    /// Objects are darker than the background.
    pub dark: bool,
    /// The value objects are measured against when dark (the signal is `flip - value`).
    pub flip: f32,
}

impl Signal {
    /// True for stain amounts from colour deconvolution.
    fn deconvolved(&self) -> bool {
        matches!(
            self.stain,
            Some(Stain::Hematoxylin | Stain::Eosin | Stain::Dab)
        )
    }

    /// The bright-object signal of one tile.
    fn of(&self, comps: &[Vec<f32>]) -> Vec<f32> {
        let n = comps.first().map_or(0, Vec::len);
        let base: Vec<f32> = match self.stain {
            None => comps[0].clone(),
            Some(Stain::Red) => comps[0].clone(),
            Some(Stain::Green) => comps[1.min(comps.len() - 1)].clone(),
            Some(Stain::Blue) => comps[2.min(comps.len() - 1)].clone(),
            Some(Stain::Luminance) => {
                let k = comps.len().min(3);
                (0..n)
                    .map(|i| comps[..k].iter().map(|c| c[i]).sum::<f32>() / k as f32)
                    .collect()
            }
            Some(Stain::Auto) => (0..n)
                .map(|i| comps.iter().take(3).map(|c| c[i]).fold(f32::MIN, f32::max))
                .collect(),
            Some(s @ (Stain::Hematoxylin | Stain::Eosin | Stain::Dab)) => {
                return stains(comps, self.i0)[stain_index(s)].clone();
            }
        };
        if self.dark {
            base.iter().map(|v| self.flip - v).collect()
        } else {
            base
        }
    }
}

fn stain_index(s: Stain) -> usize {
    match s {
        Stain::Eosin => 1,
        Stain::Dab => 2,
        _ => 0,
    }
}

/// Hematoxylin, eosin and DAB amounts (optical density) of each RGB pixel.
fn stains(comps: &[Vec<f32>], i0: [f32; 3]) -> [Vec<f32>; 3] {
    let n = comps[0].len();
    let inv = sg::unmixing([sg::HEMATOXYLIN, sg::EOSIN, sg::DAB]).unwrap_or([[0.0; 3]; 3]);
    let mut out = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    for i in 0..n {
        let od = [
            f64::from(sg::optical_density(comps[0][i], i0[0])),
            f64::from(sg::optical_density(comps[1][i], i0[1])),
            f64::from(sg::optical_density(comps[2][i], i0[2])),
        ];
        for (j, o) in out.iter_mut().enumerate() {
            o[i] = (od[0] * inv[0][j] + od[1] * inv[1][j] + od[2] * inv[2][j]).max(0.0) as f32;
        }
    }
    out
}

/// The share of clearly stained pixels (optical density over 0.3) whose colour needs a
/// negative amount of hematoxylin, eosin or DAB to be explained: high for other stains.
fn stain_misfit(comps: &[Vec<f32>], i0: [f32; 3]) -> f64 {
    let Some(inv) = sg::unmixing([sg::HEMATOXYLIN, sg::EOSIN, sg::DAB]) else {
        return 0.0;
    };
    let (mut stained, mut bad) = (0usize, 0usize);
    for i in (0..comps[0].len()).step_by(7) {
        let od = [0, 1, 2].map(|k| f64::from(sg::optical_density(comps[k][i], i0[k])));
        if od.iter().sum::<f64>() < 0.3 {
            continue;
        }
        stained += 1;
        let worst = (0..3)
            .map(|j| od[0] * inv[0][j] + od[1] * inv[1][j] + od[2] * inv[2][j])
            .fold(f64::INFINITY, f64::min);
        if worst < -0.15 {
            bad += 1;
        }
    }
    if stained == 0 {
        0.0
    } else {
        bad as f64 / stained as f64
    }
}

/// True for transmitted light (brightfield), where objects are darker than the background:
/// the median sits closer to the top of the range than to the bottom, or the plane has no
/// dark background at all (stained tissue filling the field). Fluorescence always has one.
fn looks_dark(v: &[f32]) -> bool {
    let (Some(p1), Some(p50), Some(p99), Some(top)) = (
        sg::quantile(v, 0.01),
        sg::quantile(v, 0.5),
        sg::quantile(v, 0.99),
        sg::quantile(v, 0.999),
    ) else {
        return false;
    };
    (p50 - p1) > 1.5 * (p99 - p50) || (top > 0.0 && p1 > 0.12 * top && p50 > 0.5 * top)
}

/// Decide what to measure from a sample tile of the image (`comps`: one plane, or three RGB).
fn signal_for(
    comps: &[Vec<f32>],
    o: &ObjectsRequest,
    notes: &mut Vec<String>,
) -> Result<(Signal, String)> {
    let rgb = comps.len() >= 3;
    if !rgb && o.stain != Stain::Auto {
        return Err(usage(
            "stain is for RGB images; this image has one value per pixel (choose a channel with select c=N)",
        ));
    }
    if rgb {
        let lum: Vec<f32> = (0..comps[0].len())
            .map(|i| (comps[0][i] + comps[1][i] + comps[2][i]) / 3.0)
            .collect();
        // colour pictures of fluorescence have a black background; transmitted light does not
        let brightfield = match o.polarity {
            Polarity::Auto => {
                looks_dark(&lum)
                    || match (sg::quantile(&lum, 0.5), sg::quantile(&lum, 0.999)) {
                        (Some(mid), Some(top)) => top > 0.0 && mid > 0.3 * top,
                        _ => false,
                    }
            }
            p => p == Polarity::Dark,
        };
        let i0 = [0, 1, 2].map(|k| sg::quantile(&comps[k], 0.99).unwrap_or(255.0).max(1.0));
        let stain = match (o.stain, brightfield) {
            (Stain::Auto, true) => Stain::Hematoxylin,
            (s, _) => s,
        };
        let deconvolved = matches!(stain, Stain::Hematoxylin | Stain::Eosin | Stain::Dab);
        if deconvolved {
            let misfit = stain_misfit(comps, i0);
            if misfit > 0.3 {
                notes.push(format!(
                    "{:.0}% of the stained pixels have colours that hematoxylin, eosin and DAB do not explain (another stain, such as a trichrome?): the {} amounts may be wrong; check a picture (preview marks=objects), or count in one colour with stain=red|green|blue",
                    misfit * 100.0,
                    serde_json::to_value(stain)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                ));
            }
        }
        let dark = brightfield && !deconvolved;
        let name = match stain {
            Stain::Auto => "brightest component".into(),
            s => serde_json::to_value(s)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
        };
        let flip = if dark {
            sg::quantile(&lum, 0.999).unwrap_or(255.0)
        } else {
            0.0
        };
        return Ok((
            Signal {
                stain: Some(stain),
                i0,
                dark,
                flip,
            },
            name,
        ));
    }
    let dark = match o.polarity {
        Polarity::Auto => looks_dark(&comps[0]),
        p => p == Polarity::Dark,
    };
    Ok((
        Signal {
            stain: None,
            i0: [0.0; 3],
            dark,
            flip: if dark {
                sg::quantile(&comps[0], 0.999).unwrap_or(0.0)
            } else {
                0.0
            },
        },
        String::new(),
    ))
}

/// Object diameter in pixels measured from the image. Candidates: the size of the regions
/// above Otsu's threshold, the scale of the strongest blobs (Laplacian of Gaussian) and, for
/// textured objects, the size of the regions in a smoothed copy. When they disagree, the one
/// whose segmentation gives the most compact objects of the expected size wins.
///
/// `range` bounds the candidates in pixels (from the physical sizes the kind can have, when
/// the pixel size is known); `typical`, the kind's typical size, competes as a last candidate.
/// Returns the diameter and whether it is the typical one.
fn measure_diameter(
    s: &[f32],
    w: usize,
    h: usize,
    kind: Kind,
    signal: &Signal,
    range: (f64, f64),
    typical: Option<f64>,
) -> Option<(f64, bool)> {
    let background = !signal.deconvolved();
    // objects narrower than 4 pixels cannot be told apart; wider than a third of the plane
    // they cannot be counted
    let plausible = |d: &f64| *d >= range.0.max(4.0) && *d <= range.1.min((w.min(h) as f64) / 3.0);
    // one candidate per size
    let distinct = |cands: &[f64], d: f64| cands.iter().all(|c| (c / d).max(d / c) > 1.3);
    let mut cands: Vec<f64> = Vec::new();
    {
        let mut add = |d: Option<f64>| {
            if let Some(d) = d.filter(plausible)
                && distinct(&cands, d)
            {
                cands.push(d);
            }
        };
        let (compact, peaks) = mask_sizes(s, w, h);
        add(compact.or(peaks));
        add(sg::blob_diameter(s, w, h));
        // textured objects (chromatin, granules) look like many small ones until smoothed;
        // not in brightfield, where the coarse scale is that of the cells around the nuclei
        if !signal.dark {
            let k = (w.min(h) / 128).max(2);
            let (small, sw, sh) = sg::shrink(s, w, h, k);
            let sm = sg::gaussian(&small, sw, sh, 1.5);
            let (compact, peaks) = mask_sizes(&sm, sw, sh);
            add(peaks.map(|d| d * k as f64));
            add(compact.map(|d| d * k as f64));
        }
    }
    let measured = cands.len();
    if let Some(t) = typical
        && distinct(&cands, t)
    {
        cands.push(t);
    }
    if cands.len() <= 1 {
        return cands.pop().map(|d| (d, measured == 0));
    }
    let q = |d: f64| {
        let mut plan = Plan::new(kind, d, background, ThresholdMethod::Otsu);
        let (ws, ww, wh, k) = working(s, w, h, &plan);
        let Some((t, _)) = sg::otsu(&scaled(&plan, k).flatten(&ws, ww, wh)) else {
            return 0.0;
        };
        plan.threshold = t;
        quality(s, w, h, &plan)
    };
    // the first (the compact regions' size) unless another is clearly better
    let mut best = (q(cands[0]), 0usize);
    for (i, &d) in cands.iter().enumerate().skip(1) {
        let qd = q(d);
        if qd > best.0 {
            best = (qd, i);
        }
    }
    Some((cands[best.1], best.1 >= measured))
}

/// How well a plan explains the plane: the share of the segmented area in objects that are
/// compact (solidity at least 0.85) and of a plausible size (a third to three times the
/// expected area). Measured on a central crop of at most about a thousand pixels a side.
fn quality(s: &[f32], w: usize, h: usize, plan: &Plan) -> f64 {
    let side = (8.0 * plan.d).max(768.0) as usize;
    let (cw, ch) = (w.min(side), h.min(side));
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let crop: Vec<f32> = (0..ch)
        .flat_map(|y| s[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw].iter().copied())
        .collect();
    let (lab, _) = label_tile(&crop, cw, ch, plan);
    let expected = std::f64::consts::PI * plan.d * plan.d / 4.0;
    let (mut good, mut all) = (0.0, 0.0);
    for (area, solidity) in shapes(&lab, cw, ch) {
        all += area;
        let r = area / expected;
        if solidity >= 0.85 && (0.3..=3.0).contains(&r) {
            good += area;
        }
    }
    if all > 0.0 { good / all } else { 0.0 }
}

/// Area and solidity of each labelled object.
fn shapes(lab: &[u32], w: usize, h: usize) -> Vec<(f64, f64)> {
    let n = lab.iter().copied().max().unwrap_or(0) as usize;
    let mut area = vec![0usize; n + 1];
    let mut pts: Vec<Vec<[f64; 2]>> = vec![Vec::new(); n + 1];
    for (i, &l) in lab.iter().enumerate() {
        if l == 0 {
            continue;
        }
        area[l as usize] += 1;
        let (x, y) = (i % w, i / w);
        let rim = x == 0
            || y == 0
            || x + 1 == w
            || y + 1 == h
            || lab[i - 1] != l
            || lab[i + 1] != l
            || lab[i - w] != l
            || lab[i + w] != l;
        if rim {
            let (fx, fy) = (x as f64, y as f64);
            pts[l as usize].extend([
                [fx, fy],
                [fx + 1.0, fy],
                [fx, fy + 1.0],
                [fx + 1.0, fy + 1.0],
            ]);
        }
    }
    (1..=n)
        .filter(|l| area[*l] > 0)
        .map(|l| {
            let hull = sg::hull_area(&mut pts[l]);
            let a = area[l] as f64;
            (a, if hull > 0.0 { (a / hull).min(1.0) } else { 1.0 })
        })
        .collect()
}

/// Two sizes of the regions above Otsu's threshold: the median diameter of the compact ones
/// (when there are enough), and twice the median depth of the distance-transform peaks (one
/// per object even where objects touch).
fn mask_sizes(s: &[f32], w: usize, h: usize) -> (Option<f64>, Option<f64>) {
    let sm = sg::gaussian(s, w, h, 1.0);
    let Some((t, _)) = sg::otsu(&sm) else {
        return (None, None);
    };
    let mut fg: Vec<bool> = sm.iter().map(|v| *v > t).collect();
    sg::fill_holes(&mut fg, w, h, (w * h) / 100);
    let (lab, n) = sg::components(&fg, w, h);
    if n == 0 {
        return (None, None);
    }
    let mut compact = Vec::new();
    let mut all = 0usize;
    for (area, solidity) in shapes(&lab, w, h) {
        if area < 12.0 {
            continue;
        }
        all += 1;
        if solidity >= 0.85 {
            compact.push(2.0 * (area / std::f64::consts::PI).sqrt());
        }
    }
    let compact = if compact.len() >= 5 && compact.len() * 3 >= all {
        median(&mut compact)
    } else {
        None
    };
    let dist = sg::distance(&fg, w, h);
    let (seeds, k) = sg::seeds(&dist, &fg, w, h, 3, 2.0);
    let mut depth = vec![0f32; k as usize + 1];
    for (i, &l) in seeds.iter().enumerate() {
        if l > 0 {
            depth[l as usize] = depth[l as usize].max(dist[i]);
        }
    }
    let mut d: Vec<f64> = depth
        .iter()
        .skip(1)
        .map(|r| 2.0 * f64::from(*r) * 1.1)
        .collect();
    (compact, median(&mut d))
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(super::median_sorted(v))
}

/// Every setting of a count, fixed before the tiles are read.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub kind: Kind,
    /// Expected diameter in pixels.
    pub d: f64,
    pub split: bool,
    /// Applies to [`Plan::flatten`]'s output.
    pub threshold: f32,
    pub method: ThresholdMethod,
    pub min_area: usize,
    pub max_area: usize,
    /// Remove the slowly varying background first. Stain amounts from colour deconvolution
    /// are already relative to empty glass and keep theirs.
    pub background: bool,
}

impl Plan {
    fn new(kind: Kind, d: f64, background: bool, method: ThresholdMethod) -> Plan {
        let expected = std::f64::consts::PI * d * d / 4.0;
        Plan {
            kind,
            d,
            split: kind != Kind::Spots,
            threshold: 0.0,
            method,
            min_area: if kind == Kind::Spots {
                1
            } else {
                (expected / 5.0).max(1.0) as usize
            },
            max_area: usize::MAX,
            background,
        }
    }

    /// What the threshold applies to: the signal smoothed at a twentieth of the object size,
    /// less the background under objects of that size (spots: the height above the local
    /// background).
    pub(crate) fn flatten(&self, s: &[f32], w: usize, h: usize) -> Vec<f32> {
        if self.kind == Kind::Spots {
            let sm = sg::gaussian(s, w, h, spot_sigma(self.d).min(1.0));
            let bg = sg::background(&sm, w, h, (self.d * 3.0).max(6.0));
            return sm.iter().zip(&bg).map(|(a, b)| a - b).collect();
        }
        let sm = sg::gaussian(s, w, h, (self.d / 20.0).max(0.6));
        if !self.background {
            return sm;
        }
        let bg = sg::background(&sm, w, h, self.d);
        sm.iter().zip(&bg).map(|(a, b)| a - b).collect()
    }
}

/// Laplacian-of-Gaussian scale for spots of diameter `d`.
fn spot_sigma(d: f64) -> f64 {
    (d / (2.0 * std::f64::consts::SQRT_2)).max(0.7)
}

/// Object diameter (pixels) above which a tile is labelled at a reduced scale: the steps
/// need only about 32 pixels per object, and large objects would make them slow.
const LARGE_OBJECTS: f64 = 64.0;

/// The factor [`working`] shrinks by.
fn working_factor(p: &Plan) -> usize {
    if p.kind == Kind::Spots || p.d < LARGE_OBJECTS {
        1
    } else {
        (p.d / 32.0).floor().max(1.0) as usize
    }
}

/// The plane the steps work on: `s` itself, or for large objects a copy shrunk by a whole
/// factor (returned last) with the plan scaled to match.
fn working(s: &[f32], w: usize, h: usize, p: &Plan) -> (Vec<f32>, usize, usize, usize) {
    let f = working_factor(p);
    if f == 1 {
        return (s.to_vec(), w, h, 1);
    }
    let (small, sw, sh) = sg::shrink(s, w, h, f);
    (small, sw, sh, f)
}

/// `p` for a plane shrunk by `k`.
fn scaled(p: &Plan, k: usize) -> Plan {
    let mut q = p.clone();
    if k > 1 {
        q.d = p.d / k as f64;
        q.min_area = (p.min_area / (k * k)).max(1);
    }
    q
}

/// The label image of one tile (`s`: the bright-object signal) and, for spots, each spot's
/// height above the background.
pub(crate) fn label_tile(s: &[f32], w: usize, h: usize, p: &Plan) -> (Vec<u32>, Vec<f32>) {
    if p.kind == Kind::Spots {
        return spots(s, w, h, p);
    }
    let (ws, sw, sh, f) = working(s, w, h, p);
    if f == 1 {
        return (label_objects(&ws, w, h, p), Vec::new());
    }
    // label the shrunken copy, then give each pixel the label of its block
    let lab_s = label_objects(&ws, sw, sh, &scaled(p, f));
    let mut lab = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            lab[y * w + x] = lab_s[(y / f) * sw + x / f];
        }
    }
    (lab, Vec::new())
}

/// Nuclei and cells: threshold the flattened signal, fill holes smaller than half an object,
/// split touching objects by a watershed of the distance to the background (and to dark
/// seams) from one seed per distance peak, and merge specks under the smallest size into
/// their neighbour.
fn label_objects(s: &[f32], w: usize, h: usize, p: &Plan) -> Vec<u32> {
    let f = p.flatten(s, w, h);
    let mut fg = foreground(&f, w, h, p);
    let expected = std::f64::consts::PI * p.d * p.d / 4.0;
    sg::fill_holes(&mut fg, w, h, (expected * 0.5) as usize);
    let mut lab = if p.split {
        // dark seams between touching objects: valleys narrower than a fifth of an object
        // where the signal falls by 40% or more from the ridges around them (a black
        // top-hat); cut there before measuring the distance to the background
        let r = ((p.d * 0.1).round() as usize).max(1);
        let closed = sg::rank_filter(&sg::rank_filter(&f, w, h, r, true), w, h, r, false);
        let t = p.threshold.abs().max(f32::EPSILON);
        let cut: Vec<bool> = fg
            .iter()
            .zip(closed.iter().zip(&f))
            .map(|(g, (c, v))| *g && (c - v) < 0.4 * c.max(t))
            .collect();
        let dist = sg::distance(&cut, w, h);
        // seeds: peaks of the smoothed distance at least a quarter diameter apart
        let m = sg::gaussian(&dist, w, h, (p.d * 0.08).max(0.7));
        let radius = ((p.d * 0.25).round() as usize).max(1);
        let floor = (p.d * 0.05) as f32;
        let (seeds, _) = sg::seeds(&m, &fg, w, h, radius, floor);
        sg::watershed(&m, &fg, &seeds, w, h)
    } else {
        sg::components(&fg, w, h).0
    };
    sg::tidy_labels(&mut lab, w, h, p.min_area);
    lab
}

fn foreground(f: &[f32], w: usize, h: usize, p: &Plan) -> Vec<bool> {
    if p.method == ThresholdMethod::Local {
        // against the neighbourhood mean (twice the object size), never below the global
        // threshold's half
        let local = sg::gaussian(f, w, h, p.d);
        return f
            .iter()
            .zip(&local)
            .map(|(v, l)| *v > l.max(0.0) + 0.5 * p.threshold.max(0.0) && *v > 0.5 * p.threshold)
            .collect();
    }
    f.iter().map(|v| *v > p.threshold).collect()
}

/// Spots: local maxima of the Laplacian of Gaussian at the spot scale whose height above the
/// local background reaches the threshold. Each spot is labelled as a disc of the expected
/// diameter.
fn spots(s: &[f32], w: usize, h: usize, p: &Plan) -> (Vec<u32>, Vec<f32>) {
    let resp = sg::log_filter(s, w, h, spot_sigma(p.d));
    let height = p.flatten(s, w, h);
    let fg = vec![true; w * h];
    let r = (p.d / 2.0).ceil().max(1.0) as usize;
    let (peaks, n) = sg::seeds(&resp, &fg, w, h, r, f32::MIN_POSITIVE);
    // one point per peak (plateaus: the first pixel)
    let mut first = vec![usize::MAX; n as usize + 1];
    for (i, &l) in peaks.iter().enumerate() {
        if l > 0 && first[l as usize] == usize::MAX {
            first[l as usize] = i;
        }
    }
    let mut lab = vec![0u32; w * h];
    let mut heights = Vec::new();
    let rr = (p.d / 2.0).max(1.0);
    for &i in first.iter().skip(1) {
        if i == usize::MAX || height[i] < p.threshold {
            continue;
        }
        let id = heights.len() as u32 + 1;
        heights.push(height[i]);
        let (cx, cy) = ((i % w) as isize, (i / w) as isize);
        let ri = rr.ceil() as isize;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                    continue;
                }
                if ((dx * dx + dy * dy) as f64) <= rr * rr + 0.5 {
                    let j = y as usize * w + x as usize;
                    if lab[j] == 0 {
                        lab[j] = id;
                    }
                }
            }
        }
    }
    (lab, heights)
}

/// The threshold for `plan` from a sample of the flattened signal, and its name. `auto` tries
/// Otsu's two-class threshold and the upper of Otsu's three-class thresholds (objects that
/// stand out from a middle class such as cytoplasm), and keeps the three-class one only when
/// its objects on the probe tile are clearly more compact and better sized.
fn settle_threshold(
    probe: &[f32],
    w: usize,
    h: usize,
    plan: &Plan,
    sample: &[f32],
) -> Option<(f32, &'static str)> {
    if plan.kind == Kind::Spots {
        // spots: a multiple of the background noise (robust standard deviation)
        let med = sg::quantile(sample, 0.5)?;
        let dev: Vec<f32> = sample.iter().map(|v| (v - med).abs()).collect();
        let mad = sg::quantile(&dev, 0.5)?;
        let sd = (1.4826 * mad).max(f32::EPSILON);
        return Some((med + 5.0 * sd, "noise"));
    }
    match plan.method {
        ThresholdMethod::Otsu => sg::otsu(sample).map(|t| (t.0, "otsu")),
        ThresholdMethod::Local => sg::otsu(sample).map(|t| (t.0, "local")),
        ThresholdMethod::Triangle => sg::triangle(sample).map(|t| (t, "triangle")),
        ThresholdMethod::Auto => {
            let (t, _) = sg::otsu(sample)?;
            // each class boundary moved 30% of the way towards the class below, so that dim
            // objects are whole and bright ones not much enlarged
            let k = 0.7f32;
            let med = sg::quantile(sample, 0.5)?;
            let mut cands: Vec<(f32, &'static str)> = vec![(med + k * (t - med), "otsu")];
            if let Some((lo, hi)) = sg::otsu3(sample)
                && hi > t
            {
                cands.push((lo + k * (hi - lo), "otsu-3"));
            }
            if cands.len() == 1 {
                return cands.pop();
            }
            let mut best = (f64::NEG_INFINITY, cands[0]);
            for c in cands {
                let mut p = plan.clone();
                p.threshold = c.0;
                let q = quality(probe, w, h, &p);
                // the three-class threshold only when clearly better
                if q > best.0 + 0.2 {
                    best = (q, c);
                }
            }
            Some(best.1)
        }
    }
}

/// A channel name for the label `c=N NAME`.
fn channel_label(tg: &Target<'_>, c: u32) -> Option<String> {
    tg.im
        .channels
        .iter()
        .find(|ch| ch.index == c)
        .and_then(|ch| ch.name.clone().or_else(|| ch.fluorophore.clone()))
}

/// The channel to count nuclei in when none is selected: one named like a DNA stain.
fn nuclear_channel(tg: &Target<'_>) -> Option<u32> {
    const DNA: [&str; 14] = [
        "dapi", "hoechst", "draq5", "draq7", "nucblue", "sir-dna", "sirdna", "spy-dna", "spydna",
        "nuclear", "nuclei", "h2b", "to-pro", "topro",
    ];
    tg.im
        .channels
        .iter()
        .find(|ch| {
            [&ch.name, &ch.fluorophore].iter().any(|n| {
                n.as_deref()
                    .is_some_and(|n| DNA.iter().any(|d| n.to_ascii_lowercase().contains(d)))
            })
        })
        .map(|ch| ch.index)
}

/// Per-object sums while tiles are read.
#[derive(Default, Clone)]
struct Acc {
    n: u64,
    sx: f64,
    sy: f64,
    sums: Vec<f64>,
    edge: bool,
    hull: Vec<[f64; 2]>,
    height: Option<f64>,
    outline: Vec<[f64; 2]>,
}

/// The planes of one tile: the components of the counted channel and every channel measured.
struct TileData {
    comps: Vec<Vec<f32>>,
    /// One plane per measured channel (RGB brightfield: the three stains).
    measured: Vec<Vec<f32>>,
}

fn grid_f32(g: &Grid) -> Vec<Vec<f32>> {
    g.comps
        .iter()
        .map(|c| c.iter().map(|v| *v as f32).collect())
        .collect()
}

pub(crate) fn objects(
    ds: &mut dyn crate::reader::Dataset,
    tg: &Target<'_>,
    req: &super::MeasureRequest,
    o: &ObjectsRequest,
    notes: &mut Vec<String>,
) -> Result<Objects> {
    let poly = req.polygon.as_deref();
    let r = match (req.region, poly) {
        (Some(r), _) => {
            r.check_within(
                tg.level_w,
                tg.level_h,
                &format!("image {} level {}", tg.im.index, req.level),
            )?;
            r
        }
        (None, Some(p)) => super::bounds(p, 0.0, tg.level_w, tg.level_h)?,
        (None, None) => Region::full(tg.level_w, tg.level_h),
    };
    let total = u64::from(r.width) * u64::from(r.height);
    if total > MAX_OBJECT_PIXELS {
        let coarser = (1..tg.im.pyramid_levels.max(1))
            .find(|l| total >> (2 * l) <= MAX_OBJECT_PIXELS)
            .map(|l| format!(" (level {})", req.level + l))
            .unwrap_or_default();
        return Err(Error::unsupported(
            "stats",
            format!("an object count over {} x {} pixels", r.width, r.height),
            format!(
                "Count objects in at most {MAX_OBJECT_PIXELS} pixels: pass a region (or polygon) inside it, or a coarser level{coarser}; coordinates are then that level's pixels."
            ),
        ));
    }
    // the channel objects are found in
    let selected_c = !req.select.iter().all(|s| !s.contains('c'));
    let c = if selected_c {
        tg.channels.first().copied().unwrap_or(0)
    } else if o.kind == Kind::Nuclei {
        nuclear_channel(tg).unwrap_or(0)
    } else {
        0
    };
    let counted = c;
    let all_channels: Vec<u32> = (0..tg.im.size_c.max(1)).take(8).collect();
    if tg.im.size_c > 1 && !selected_c {
        notes.push(format!(
            "objects were found in channel {c}{}; select c=N for another",
            channel_label(tg, c)
                .map(|n| format!(" ({n})"))
                .unwrap_or_default()
        ));
    }
    let pixel_um = tg.pixel_um;
    let area_px_um2 = pixel_um.map(|p| p * tg.pixel_um_y.unwrap_or(p));
    // probe: the tile at the centre decides polarity, stain and diameter
    let probe = centre_tile(r);
    let g = read(ds, tg, req.level, c, probe)?;
    let comps = grid_f32(&g);
    let rgb = comps.len() >= 3;
    let (signal, stain_name) = signal_for(&comps, o, notes)?;
    let measured = if rgb {
        stain_name
    } else {
        format!(
            "c={c}{}",
            channel_label(tg, c)
                .map(|n| format!(" {n}"))
                .unwrap_or_default()
        )
    };
    let s = signal.of(&comps);
    let background = !signal.deconvolved();
    let (d, from) = diameter(o, &s, probe, pixel_um, &signal, notes)?;
    if d < 3.0 && o.kind != Kind::Spots {
        notes.push(format!(
            "objects are about {d:.1} px across at this level: too small to separate; read a finer level"
        ));
    }
    let to_px2 = |um2: f64| area_px_um2.map(|a| um2 / a);
    let expected = std::f64::consts::PI * d * d / 4.0;
    let min_area = match (o.min_area_um2, to_px2(1.0)) {
        (Some(a), Some(_)) => to_px2(a).unwrap_or(1.0),
        (Some(_), None) => {
            return Err(usage(
                "min_area_um2 needs the pixel size, which this image does not record; set diameter_px instead (objects under a fifth of its area are dropped)",
            ));
        }
        (None, _) if o.kind == Kind::Spots => 1.0,
        (None, _) => expected / 5.0,
    }
    .max(1.0) as usize;
    let max_area = match (o.max_area_um2, to_px2(1.0)) {
        (Some(a), Some(_)) => to_px2(a).unwrap_or(f64::MAX),
        (Some(_), None) => {
            return Err(usage(
                "max_area_um2 needs the pixel size, which this image does not record",
            ));
        }
        (None, _) => f64::MAX,
    };
    let max_area = if max_area >= usize::MAX as f64 {
        usize::MAX
    } else {
        max_area as usize
    };
    // threshold: from the whole area (tile by tile when it is large)
    let tiles = tiles(r, d);
    let mut plan = Plan::new(o.kind, d, background, o.method);
    plan.split &= o.split.unwrap_or(true);
    plan.min_area = min_area;
    plan.max_area = max_area;
    let method_name;
    if let Some(t) = o.threshold {
        plan.threshold = t as f32;
        method_name = "given".to_string();
    } else {
        let mut sample: Vec<f32> = Vec::new();
        let single = tiles.len() == 1;
        for t in &tiles {
            let s = if single && t.read == probe {
                s.clone()
            } else {
                let g = read(ds, tg, req.level, c, t.read)?;
                signal.of(&grid_f32(&g))
            };
            let (w, h) = (t.read.width as usize, t.read.height as usize);
            let (ws, sw, _, k) = working(&s, w, h, &plan);
            let f = scaled(&plan, k).flatten(&ws, sw, ws.len() / sw.max(1));
            // every pixel of one tile, every third of many
            let stride = if single { 1 } else { 3 };
            let core = t.core_in_read();
            let k32 = k as u32;
            for y in (core.y / k32..(core.y + core.height) / k32).step_by(stride) {
                for x in (core.x / k32..(core.x + core.width) / k32).step_by(stride) {
                    let i = y as usize * sw + x as usize;
                    let (gx, gy) = (t.read.x + x * k32, t.read.y + y * k32);
                    if i < f.len()
                        && poly.is_none_or(|p| inside(p, f64::from(gx) + 0.5, f64::from(gy) + 0.5))
                    {
                        sample.push(f[i]);
                    }
                }
            }
        }
        let (pw, ph) = (probe.width as usize, probe.height as usize);
        let (t, m) = settle_threshold(&s, pw, ph, &plan, &sample)
            .ok_or_else(|| usage("the area is flat (one value): there is nothing to count"))?;
        if o.kind != Kind::Spots
            && let Some((_, sep)) = sg::otsu(&sample)
            && sep < 0.5
        {
            notes.push(format!(
                        "weak contrast between objects and background (Otsu separability {sep:.2}): the count may be unreliable; check a picture (preview marks=objects)"
                    ));
        }
        plan.threshold = t;
        method_name = m.to_string();
    }
    // the objects, tile by tile
    let n_meas = if rgb { 3 } else { all_channels.len() };
    let mut accs: Vec<Acc> = Vec::new();
    let mut bg_samples: Vec<Vec<f32>> = vec![Vec::new(); n_meas];
    let mut covered = 0u64;
    let mut searched = 0u64;
    for t in &tiles {
        let data = tile_data(ds, tg, req.level, c, &all_channels, t.read, &signal)?;
        let (w, h) = (t.read.width as usize, t.read.height as usize);
        let s = signal.of(&data.comps);
        let (lab, heights) = label_tile(&s, w, h, &plan);
        let n = lab.iter().copied().max().unwrap_or(0) as usize;
        let mut a: Vec<Acc> = vec![
            Acc {
                sums: vec![0.0; n_meas],
                ..Acc::default()
            };
            n + 1
        ];
        let mut first = vec![usize::MAX; n + 1];
        for (i, &l) in lab.iter().enumerate() {
            let (x, y) = (i % w, i / w);
            let (gx, gy) = (t.read.x + x as u32, t.read.y + y as u32);
            let in_core = t.core.contains_px(gx, gy);
            let in_poly = poly.is_none_or(|p| inside(p, f64::from(gx) + 0.5, f64::from(gy) + 0.5));
            if in_core && in_poly {
                searched += 1;
            }
            if l == 0 {
                if in_core && in_poly && (x + y * 7) % 5 == 0 {
                    for (k, m) in data.measured.iter().enumerate() {
                        bg_samples[k].push(m[i]);
                    }
                }
                continue;
            }
            let l = l as usize;
            if first[l] == usize::MAX {
                first[l] = i;
            }
            let e = &mut a[l];
            e.n += 1;
            e.sx += x as f64;
            e.sy += y as f64;
            for (k, m) in data.measured.iter().enumerate() {
                e.sums[k] += f64::from(m[i]);
            }
            // the object touches the edge of what is searched
            e.edge |= gx == r.x
                || gy == r.y
                || gx + 1 == r.x + r.width
                || gy + 1 == r.y + r.height
                || poly.is_some() && !in_poly;
            let (fx, fy) = (gx as f64, gy as f64);
            let at_rim = x == 0
                || y == 0
                || x + 1 == w
                || y + 1 == h
                || lab[i - 1] != lab[i]
                || lab[i + 1] != lab[i]
                || lab[i - w] != lab[i]
                || lab[i + w] != lab[i];
            if at_rim {
                e.hull.extend([
                    [fx, fy],
                    [fx + 1.0, fy],
                    [fx, fy + 1.0],
                    [fx + 1.0, fy + 1.0],
                ]);
            }
        }
        for (l, e) in a.iter_mut().enumerate().skip(1) {
            if e.n == 0 {
                continue;
            }
            let (cx, cy) = (
                f64::from(t.read.x) + e.sx / e.n as f64 + 0.5,
                f64::from(t.read.y) + e.sy / e.n as f64 + 0.5,
            );
            // each object belongs to the tile whose core holds its centroid
            if !t.core.contains_px(cx as u32, cy as u32) {
                continue;
            }
            if poly.is_some_and(|p| !inside(p, cx, cy)) {
                continue;
            }
            if (e.n as usize) > plan.max_area {
                continue;
            }
            // cut by the tile margin (not the area's edge): the margin is two diameters wide,
            // so this only happens to very large objects
            covered += e.n;
            e.sx = cx;
            e.sy = cy;
            e.height = heights.get(l - 1).map(|h| f64::from(*h));
            if o.outlines {
                // labels found on a shrunken copy have block-sized steps: smooth them away
                let block = working_factor(&plan);
                let mut ol = sg::outline(&lab, w, h, l as u32, first[l], 0.4 + 0.6 * block as f64);
                if block > 1 {
                    ol = sg::chaikin(&ol, 2);
                }
                for p in &mut ol {
                    p[0] += f64::from(t.read.x);
                    p[1] += f64::from(t.read.y);
                }
                e.outline = ol;
            }
            accs.push(std::mem::take(e));
        }
    }
    // reading order: rows of about one object height, then left to right
    let band = d.max(1.0);
    accs.sort_by(|a, b| {
        ((a.sy / band).floor())
            .total_cmp(&(b.sy / band).floor())
            .then(a.sx.total_cmp(&b.sx))
    });
    let stained = rgb && (signal.dark || signal.deconvolved());
    let channels_meta: Vec<(u32, Option<String>)> = if stained {
        vec![
            (0, Some("hematoxylin".into())),
            (1, Some("eosin".into())),
            (2, Some("dab".into())),
        ]
    } else if rgb {
        vec![
            (0, Some("R".into())),
            (1, Some("G".into())),
            (2, Some("B".into())),
        ]
    } else {
        all_channels
            .iter()
            .map(|c| (*c, channel_label(tg, *c)))
            .collect()
    };
    let mut all: Vec<Object> = accs
        .into_iter()
        .enumerate()
        .map(|(i, mut e)| {
            let solidity = (o.kind != Kind::Spots)
                .then(|| {
                    let hull = sg::hull_area(&mut e.hull);
                    (hull > 0.0).then(|| (e.n as f64 / hull).min(1.0))
                })
                .flatten();
            let area_um2 = area_px_um2.map(|a| e.n as f64 * a);
            Object {
                id: i as u32 + 1,
                x: (e.sx * 10.0).round() / 10.0,
                y: (e.sy * 10.0).round() / 10.0,
                area_px: e.n,
                area_um2: area_um2.map(round4),
                diameter_um: area_um2.map(|a| round4(2.0 * (a / std::f64::consts::PI).sqrt())),
                solidity: solidity.map(|s| (s * 100.0).round() / 100.0),
                height: e.height.map(round4),
                mean: e.sums.iter().map(|s| round4(s / e.n as f64)).collect(),
                edge: e.edge,
                outline: e.outline,
            }
        })
        .collect();
    let count = all.len();
    let touching = all.iter().filter(|o| o.edge).count();
    let inner: Vec<&Object> = if touching < count {
        all.iter().filter(|o| !o.edge).collect()
    } else {
        all.iter().collect()
    };
    let areas: Vec<f64> = inner.iter().map(|o| o.area_px as f64).collect();
    let area_px = spread(areas.clone());
    let med = area_px.as_ref().map_or(0.0, |s| s.median);
    let large = if o.kind == Kind::Spots {
        0
    } else {
        all.iter().filter(|o| o.area_px as f64 > 2.0 * med).count()
    };
    let area_um2 = area_px_um2.and_then(|a| spread(areas.iter().map(|x| x * a).collect()));
    let diameter_um_spread = area_px_um2.and_then(|a| {
        spread(
            areas
                .iter()
                .map(|x| 2.0 * (x * a / std::f64::consts::PI).sqrt())
                .collect(),
        )
    });
    let searched_um2 = area_px_um2.map(|a| searched as f64 * a);
    let density = searched_um2
        .filter(|a| *a > 0.0)
        .map(|a| round4((count as f64 - touching as f64 / 2.0) / (a / 1e6)));
    let channels = channels_meta
        .into_iter()
        .enumerate()
        .map(|(k, (c, name))| {
            let means: Vec<f64> = all.iter().map(|o| o.mean[k]).collect();
            let bg = &bg_samples[k];
            let background = sg::quantile(bg, 0.5).map_or(f64::NAN, f64::from);
            // positive calls for markers: other channels than the one counted, and DAB
            let marker = if stained { k == 2 } else { rgb || c != counted };
            let positive = marker.then(|| positive(&means, bg)).flatten();
            ObjectChannel {
                c,
                name,
                background: round4(background),
                mean: spread(means).map(round_spread),
                positive,
            }
        })
        .collect();
    // say when the objects do not look like what was asked for
    if o.kind != Kind::Spots && count >= 10 {
        let mut sol: Vec<f64> = all.iter().filter_map(|o| o.solidity).collect();
        let irregular = median(&mut sol).is_some_and(|s| s < 0.8);
        if large * 4 > count || irregular {
            notes.push(format!(
                "{} of {count} objects are over twice the median size{}: touching objects were not all separated, or the stain marks more than {}; check a picture (preview marks=objects) and set diameter_um or another channel or stain",
                large,
                if irregular { " and most are irregular" } else { "" },
                kind_name(o.kind)
            ));
        }
    }
    if count == 0 {
        notes.push(format!(
            "no {} found above the threshold; check a picture, or set diameter_um, threshold or polarity",
            kind_name(o.kind)
        ));
    }
    let max_list = o.max_objects.unwrap_or(DEFAULT_MAX_OBJECTS);
    let truncated = count > max_list;
    all.truncate(max_list);
    let d_um = pixel_um.map(|p| round4(d * p));
    Ok(Objects {
        kind: o.kind,
        measured,
        region: r,
        searched_um2: searched_um2.map(round4),
        expected_diameter_px: round4(d),
        expected_diameter_um: d_um,
        expected_diameter_from: from.into(),
        polarity: if signal.dark || signal.deconvolved() {
            Polarity::Dark
        } else {
            Polarity::Bright
        },
        threshold: round4(f64::from(plan.threshold)),
        threshold_method: method_name,
        split: plan.split,
        count,
        touching_edge: touching,
        density_per_mm2: density,
        area_fraction: round4(covered as f64 / searched.max(1) as f64),
        large,
        area_px: area_px.map(round_spread),
        area_um2: area_um2.map(round_spread),
        diameter_um: diameter_um_spread.map(round_spread),
        channels,
        objects: all,
        truncated,
    })
}

fn round4(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let d = 4 - 1 - v.abs().log10().floor() as i32;
    if d <= 0 {
        return v.round();
    }
    let m = 10f64.powi(d.min(12));
    (v * m).round() / m
}

fn round_spread(s: Spread) -> Spread {
    Spread {
        min: round4(s.min),
        median: round4(s.median),
        mean: round4(s.mean),
        max: round4(s.max),
    }
}

/// Which objects are clearly brighter in a channel: Otsu's split of the object means when they
/// fall in two well-separated groups, else those above the background by five robust
/// standard deviations of the background (and at least a tenth of the way to the brightest
/// objects, for backgrounds without noise).
fn positive(means: &[f64], bg: &[f32]) -> Option<Positive> {
    if means.is_empty() {
        return None;
    }
    let m32: Vec<f32> = means.iter().map(|v| *v as f32).collect();
    let bg_med = sg::quantile(bg, 0.5)?;
    let dev: Vec<f32> = bg.iter().map(|v| (v - bg_med).abs()).collect();
    let bg_sd = 1.4826 * sg::quantile(&dev, 0.5)?;
    let top = sg::quantile(&m32, 0.95)?;
    let floor = f64::from(bg_med + (5.0 * bg_sd).max(0.1 * (top - bg_med)).max(f32::EPSILON));
    let (threshold, method) = match sg::otsu(&m32) {
        // two groups, the upper one above the background
        Some((t, sep)) if sep >= 0.75 && means.len() >= 8 && f64::from(t) > floor => {
            (f64::from(t), "otsu")
        }
        _ => (floor, "background"),
    };
    let count = means.iter().filter(|m| **m > threshold).count();
    Some(Positive {
        threshold: round4(threshold),
        method: if count == 0 {
            "none".into()
        } else {
            method.into()
        },
        count,
        fraction: round4(count as f64 / means.len() as f64),
    })
}

/// Read the counted channel and every measured channel of one tile.
fn tile_data(
    ds: &mut dyn crate::reader::Dataset,
    tg: &Target<'_>,
    level: u32,
    c: u32,
    all: &[u32],
    r: Region,
    signal: &Signal,
) -> Result<TileData> {
    let g = read(ds, tg, level, c, r)?;
    let comps = grid_f32(&g);
    let measured = if comps.len() >= 3 {
        if signal.dark
            || matches!(
                signal.stain,
                Some(Stain::Hematoxylin | Stain::Eosin | Stain::Dab)
            )
        {
            let i0 = if signal.i0.iter().all(|v| *v <= 0.0) {
                [255.0; 3]
            } else {
                signal.i0
            };
            stains(&comps, i0).to_vec()
        } else {
            comps.iter().take(3).cloned().collect()
        }
    } else {
        let mut m = Vec::with_capacity(all.len());
        for &k in all {
            if k == c {
                m.push(comps[0].clone());
            } else {
                m.push(grid_f32(&read(ds, tg, level, k, r)?).swap_remove(0));
            }
        }
        m
    };
    Ok(TileData { comps, measured })
}

/// The expected diameter in pixels and where it came from.
fn diameter(
    o: &ObjectsRequest,
    s: &[f32],
    probe: Region,
    pixel_um: Option<f64>,
    signal: &Signal,
    notes: &mut Vec<String>,
) -> Result<(f64, &'static str)> {
    let check = |d: f64, what: &str| -> Result<f64> {
        if d.is_finite() && d > 0.0 {
            Ok(d)
        } else {
            Err(usage(format!("{what} must be a positive number")))
        }
    };
    if let Some(d) = o.diameter_um {
        let d = check(d, "diameter_um")?;
        let Some(p) = pixel_um else {
            return Err(usage(
                "diameter_um needs the pixel size, which this image does not record; give diameter_px instead",
            ));
        };
        return Ok((d / p, "given"));
    }
    if let Some(d) = o.diameter_px {
        return Ok((check(d, "diameter_px")?, "given"));
    }
    // typical sizes and the range a measurement may fall in, in µm
    let (typical, lo, hi) = match o.kind {
        Kind::Nuclei => (10.0, 3.0, 30.0),
        Kind::Cells => (20.0, 6.0, 150.0),
        Kind::Spots => (0.5, 0.1, 3.0),
    };
    if o.kind == Kind::Spots {
        return Ok(match pixel_um {
            Some(p) => ((typical / p).max(2.0), "default"),
            None => (3.0, "default"),
        });
    }
    let (w, h) = (probe.width as usize, probe.height as usize);
    let (range, typical_px) = match pixel_um {
        Some(p) => ((lo / p, hi / p), Some(typical / p)),
        None => ((0.0, f64::INFINITY), None),
    };
    match (
        measure_diameter(s, w, h, o.kind, signal, range, typical_px),
        pixel_um,
    ) {
        (Some((d, false)), _) => Ok((d, "measured")),
        (Some((d, true)), _) => Ok((d, "default")),
        (None, Some(p)) => {
            notes.push(format!(
                "could not measure the {} in the image; used the typical {typical} µm (set diameter_um to override)",
                kind_name(o.kind)
            ));
            Ok((typical / p, "default"))
        }
        (None, None) => {
            notes.push("could not measure the object size and the image has no pixel size; used 20 px (set diameter_px)".into());
            Ok((20.0, "default"))
        }
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Nuclei => "nuclei",
        Kind::Cells => "cells",
        Kind::Spots => "spots",
    }
}

/// The tile at the centre of `r` (all of `r` when it is counted in one piece).
fn centre_tile(r: Region) -> Region {
    if u64::from(r.width) * u64::from(r.height) <= ONE_PIECE {
        return r;
    }
    let w = r.width.min(TILE);
    let h = r.height.min(TILE);
    Region::new(r.x + (r.width - w) / 2, r.y + (r.height - h) / 2, w, h)
}

/// One tile: the core it owns and the larger rectangle read around it.
#[derive(Debug, Clone, Copy)]
struct Tile {
    core: Region,
    read: Region,
}

impl Tile {
    /// The core in the coordinates of the rectangle read.
    fn core_in_read(&self) -> Region {
        Region::new(
            self.core.x - self.read.x,
            self.core.y - self.read.y,
            self.core.width,
            self.core.height,
        )
    }
}

trait ContainsPx {
    fn contains_px(&self, x: u32, y: u32) -> bool;
}

impl ContainsPx for Region {
    fn contains_px(&self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x - self.x < self.width && y - self.y < self.height
    }
}

/// Split `r` into tiles: one when it holds at most [`ONE_PIECE`] pixels, else cores of
/// [`TILE`] pixels a side, each read with a margin of three object diameters (clipped to `r`)
/// so that the objects a core owns are found as they would be in one piece.
fn tiles(r: Region, d: f64) -> Vec<Tile> {
    if u64::from(r.width) * u64::from(r.height) <= ONE_PIECE {
        return vec![Tile { core: r, read: r }];
    }
    let m = (3.0 * d).ceil().clamp(16.0, 1024.0) as u32;
    let mut out = Vec::new();
    let mut y = r.y;
    while y < r.y + r.height {
        let th = TILE.min(r.y + r.height - y);
        let mut x = r.x;
        while x < r.x + r.width {
            let tw = TILE.min(r.x + r.width - x);
            let core = Region::new(x, y, tw, th);
            let rx0 = x.saturating_sub(m).max(r.x);
            let ry0 = y.saturating_sub(m).max(r.y);
            let rx1 = (x + tw + m).min(r.x + r.width);
            let ry1 = (y + th + m).min(r.y + r.height);
            out.push(Tile {
                core,
                read: Region::new(rx0, ry0, rx1 - rx0, ry1 - ry0),
            });
            x += tw;
        }
        y += th;
    }
    out
}

/// Label a plane held in memory: `comps` one plane (or three RGB components), `w × h`, with
/// the pixel size in µm when known. For tests and evaluation against annotated images: the
/// label image (objects 1, 2, …; 0 = background) and the expected diameter used, in pixels.
#[doc(hidden)]
pub fn label_plane(
    comps: &[Vec<f32>],
    w: usize,
    h: usize,
    pixel_um: Option<f64>,
    o: &ObjectsRequest,
) -> Result<(Vec<u32>, f64)> {
    if comps.is_empty() || comps.iter().any(|c| c.len() != w * h) {
        return Err(usage("label_plane: each component needs w × h values"));
    }
    let mut notes = Vec::new();
    let (signal, _) = signal_for(comps, o, &mut notes)?;
    let s = signal.of(comps);
    let background = !signal.deconvolved();
    let full = Region::full(w as u32, h as u32);
    let (d, _) = diameter(o, &s, full, pixel_um, &signal, &mut notes)?;
    let mut plan = Plan::new(o.kind, d, background, o.method);
    plan.split &= o.split.unwrap_or(true);
    plan.threshold = if let Some(t) = o.threshold {
        t as f32
    } else {
        let (ws, sw, _, k) = working(&s, w, h, &plan);
        let f = scaled(&plan, k).flatten(&ws, sw, ws.len() / sw.max(1));
        settle_threshold(&s, w, h, &plan, &f)
            .ok_or_else(|| usage("the plane is flat"))?
            .0
    };
    Ok((label_tile(&s, w, h, &plan).0, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::{MeasureRequest, measure};
    use crate::model::{ChannelInfo, CheckReport, FileInfo, ImageInfo, LsEntry, PhysicalSize};
    use crate::pixel::Plane;
    use crate::reader::{Dataset, PlaneIndex};
    use crate::{PixelType, ProvenanceMap};

    /// A small random-number generator for reproducible noise.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32 - 0.5
        }
    }

    /// A `w × h` plane with discs `(cx, cy, r, value)` on `background`, plus noise.
    fn discs(
        w: usize,
        h: usize,
        d: &[(f32, f32, f32, f32)],
        background: f32,
        noise: f32,
    ) -> Vec<f32> {
        let mut rng = Lcg(7);
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let mut v = background;
                for &(cx, cy, r, val) in d {
                    if (x - cx).hypot(y - cy) <= r {
                        v = val;
                    }
                }
                v + noise * rng.next()
            })
            .collect()
    }

    fn count(lab: &[u32]) -> usize {
        let mut v: Vec<u32> = lab.iter().copied().filter(|l| *l > 0).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    }

    /// Nine nuclei of radius 10: a separate grid of six, and a chain of three that touch.
    fn nuclei() -> Vec<(f32, f32, f32, f32)> {
        let mut d = Vec::new();
        for (i, cx) in [30.0, 80.0, 130.0].into_iter().enumerate() {
            d.push((cx, 30.0, 10.0, 200.0 + 20.0 * i as f32));
            d.push((cx, 80.0, 10.0, 150.0));
        }
        d.push((40.0, 140.0, 10.0, 180.0));
        d.push((59.0, 140.0, 10.0, 190.0));
        d.push((78.0, 141.0, 10.0, 170.0));
        d
    }

    #[test]
    fn counts_and_splits_bright_nuclei() {
        let (w, h) = (180, 180);
        let plane = discs(w, h, &nuclei(), 20.0, 10.0);
        for o in [
            ObjectsRequest::default(),
            ObjectsRequest {
                diameter_px: Some(20.0),
                ..ObjectsRequest::default()
            },
        ] {
            let (lab, d) = label_plane(std::slice::from_ref(&plane), w, h, None, &o).unwrap();
            assert!((12.0..30.0).contains(&d), "{d}");
            assert_eq!(count(&lab), 9, "diameter {d}");
        }
        // kept together without the watershed
        let o = ObjectsRequest {
            diameter_px: Some(20.0),
            split: Some(false),
            ..ObjectsRequest::default()
        };
        assert_eq!(count(&label_plane(&[plane], w, h, None, &o).unwrap().0), 7);
    }

    #[test]
    fn finds_dark_nuclei_on_a_bright_background() {
        let (w, h) = (180, 180);
        let d: Vec<_> = nuclei()
            .into_iter()
            .map(|(x, y, r, v)| (x, y, r, 250.0 - v))
            .collect();
        let plane = discs(w, h, &d, 230.0, 8.0);
        let (lab, _) = label_plane(&[plane], w, h, None, &ObjectsRequest::default()).unwrap();
        assert_eq!(count(&lab), 9);
    }

    #[test]
    fn finds_hematoxylin_nuclei_in_stained_tissue() {
        // pink (eosin) tissue with purple (hematoxylin) nuclei, on glass at the top
        let (w, h) = (180, 180);
        let mut r = vec![0f32; w * h];
        let mut g = vec![0f32; w * h];
        let mut b = vec![0f32; w * h];
        let inside = discs(
            w,
            h,
            &nuclei()
                .iter()
                .map(|n| (n.0, n.1, n.2, 1.0))
                .collect::<Vec<_>>(),
            0.0,
            0.0,
        );
        let mut rng = Lcg(3);
        for i in 0..w * h {
            let (cr, cg, cb) = if i / w < 8 {
                (245.0, 245.0, 245.0)
            } else if inside[i] > 0.5 {
                (90.0, 60.0, 150.0)
            } else {
                (235.0, 150.0, 200.0)
            };
            r[i] = cr + 6.0 * rng.next();
            g[i] = cg + 6.0 * rng.next();
            b[i] = cb + 6.0 * rng.next();
        }
        let o = ObjectsRequest::default();
        let mut notes = Vec::new();
        let (sig, name) = signal_for(&[r.clone(), g.clone(), b.clone()], &o, &mut notes).unwrap();
        assert_eq!(name, "hematoxylin");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(sig.deconvolved());
        let (lab, _) = label_plane(&[r, g, b], w, h, None, &o).unwrap();
        assert_eq!(count(&lab), 9);
    }

    #[test]
    fn finds_spots_above_the_noise() {
        let (w, h) = (120, 120);
        let mut d = Vec::new();
        for i in 0..5 {
            for j in 0..4 {
                d.push((15.0 + 22.0 * i as f32, 15.0 + 28.0 * j as f32, 1.5, 300.0));
            }
        }
        let plane = discs(w, h, &d, 100.0, 20.0);
        let o = ObjectsRequest {
            kind: Kind::Spots,
            ..ObjectsRequest::default()
        };
        let (lab, _) = label_plane(&[plane], w, h, None, &o).unwrap();
        assert_eq!(count(&lab), 20);
    }

    #[test]
    fn flat_and_tiny_planes_fail_cleanly() {
        let o = ObjectsRequest::default();
        assert!(label_plane(&[vec![5.0; 100]], 10, 10, None, &o).is_err());
        assert!(label_plane(&[vec![1.0]], 1, 1, None, &o).is_err());
        assert!(label_plane(&[vec![1.0; 3]], 2, 2, None, &o).is_err());
        assert!(label_plane(&[], 0, 0, None, &o).is_err());
        let mut nan = discs(40, 40, &[(20.0, 20.0, 6.0, 100.0)], 0.0, 1.0);
        nan[5] = f32::NAN;
        let _ = label_plane(&[nan], 40, 40, None, &o);
    }

    #[test]
    fn requests_reject_unknown_options() {
        let e = serde_json::from_str::<ObjectsRequest>(r#"{"min_area_px": 4}"#);
        assert!(e.is_err());
        let o: ObjectsRequest =
            serde_json::from_str(r#"{"kind": "spots", "diameter_um": 0.4}"#).unwrap();
        assert_eq!(o.kind, Kind::Spots);
    }

    /// Two 16-bit channels: DAPI nuclei in channel 1, a marker in half of them in channel 0.
    struct TwoChannels {
        w: usize,
        h: usize,
        planes: [Vec<f32>; 2],
    }

    impl TwoChannels {
        fn new() -> TwoChannels {
            let (w, h) = (180, 180);
            let n = nuclei();
            let dapi = discs(w, h, &n, 100.0, 20.0);
            let marked: Vec<_> = n
                .iter()
                .enumerate()
                .map(|(i, d)| (d.0, d.1, d.2, if i % 2 == 0 { 3000.0 } else { 200.0 }))
                .collect();
            let marker = discs(w, h, &marked, 200.0, 30.0);
            TwoChannels {
                w,
                h,
                planes: [marker, dapi],
            }
        }

        fn file_info(&self) -> FileInfo {
            let mut im = ImageInfo::new(0, self.w as u32, self.h as u32, PixelType::Uint16);
            im.size_c = 2;
            im.physical_size = PhysicalSize::micrometres(Some(0.5), Some(0.5), None);
            im.channels = vec![
                ChannelInfo {
                    index: 0,
                    name: Some("Ki67".into()),
                    ..ChannelInfo::default()
                },
                ChannelInfo {
                    index: 1,
                    name: Some("DAPI".into()),
                    ..ChannelInfo::default()
                },
            ];
            FileInfo {
                path: "two.tif".into(),
                size_bytes: 1,
                format: crate::model::FormatDescriptor {
                    id: "tiff".into(),
                    name: "TIFF".into(),
                    vendor: String::new(),
                    extensions: vec![],
                    family: "microscopy".into(),
                    can_read: true,
                    can_write: false,
                    confidence: crate::provenance::Confidence::High,
                    known_gaps: vec![],
                },
                format_version: None,
                images: vec![im.finish()],
                tables: vec![],
                spectra: vec![],
                traces: vec![],
                plane_count: 2,
                notes: vec![],
            }
        }
    }

    impl Dataset for TwoChannels {
        fn info(&self) -> Result<FileInfo> {
            Ok(self.file_info())
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, i: PlaneIndex) -> Result<Plane> {
            let data = self.planes[i.c as usize]
                .iter()
                .flat_map(|v| (v.clamp(0.0, 65535.0) as u16).to_le_bytes())
                .collect();
            Ok(Plane {
                width: self.w as u32,
                height: self.h as u32,
                pixel_type: PixelType::Uint16,
                samples_per_pixel: 1,
                data,
            })
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("tiff", "two.tif"))
        }
    }

    #[test]
    fn one_call_counts_nuclei_and_marker_positive_cells() {
        let mut ds = TwoChannels::new();
        let info = ds.info().unwrap();
        let req = MeasureRequest {
            objects: Some(ObjectsRequest::default()),
            ..MeasureRequest::default()
        };
        let out = measure(&mut ds, &info, &req).unwrap();
        let o = out.objects.unwrap();
        assert_eq!(o.measured, "c=1 DAPI");
        assert_eq!(o.count, 9);
        assert_eq!(o.objects.len(), 9);
        assert_eq!(o.channels.len(), 2);
        let ki67 = o.channels[0].positive.as_ref().unwrap();
        assert_eq!(ki67.count, 5, "{ki67:?}");
        assert!(
            o.channels[1].positive.is_none(),
            "the counted channel has no positive call"
        );
        // 10 px radius at 0.5 µm: about 10 µm across
        let d = o.diameter_um.unwrap().median;
        assert!((8.0..12.0).contains(&d), "{d}");
        assert!(o.density_per_mm2.unwrap() > 0.0);
        // numbered in reading order
        assert!(o.objects.windows(2).all(|p| p[0].y <= p[1].y + 12.0));
        // in a polygon around the top row only
        let req = MeasureRequest {
            polygon: Some(vec![[0.0, 0.0], [180.0, 0.0], [180.0, 55.0], [0.0, 55.0]]),
            objects: Some(ObjectsRequest {
                diameter_um: Some(10.0),
                ..ObjectsRequest::default()
            }),
            ..MeasureRequest::default()
        };
        let o = measure(&mut ds, &info, &req).unwrap().objects.unwrap();
        assert_eq!(o.count, 3);
        assert_eq!(o.expected_diameter_from, "given");
    }

    #[test]
    fn tiles_count_each_object_once() {
        let r = Region::new(0, 0, 5000, 3000);
        let t = tiles(r, 20.0);
        assert_eq!(t.len(), 6);
        let covered: u64 = t
            .iter()
            .map(|t| u64::from(t.core.width) * u64::from(t.core.height))
            .sum();
        assert_eq!(covered, 5000 * 3000);
        assert!(
            t.iter()
                .all(|t| t.read.x <= t.core.x && t.read.width >= t.core.width)
        );
        assert_eq!(tiles(Region::new(0, 0, 2000, 2000), 20.0).len(), 1);
    }
}
