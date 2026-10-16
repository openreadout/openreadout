//! Lanes and bands of a gel or blot image: where the lanes are, each lane's intensity profile
//! along the direction of migration, and the bands on it with their background-subtracted
//! volumes, so "how strong is the band in lane 3 relative to lane 1" takes one call.
//!
//! Method (the definitions users see are in `book/src/reference/commands/stats.md`, "Lanes"):
//!
//! 1. **Signal.** The plane is read whole (binned by block means when it is larger than
//!    [`MAX_LANE_PIXELS`]; coordinates stay full-resolution pixels). Bright bands on a dark
//!    background (fluorescence, chemiluminescence) are measured as stored; dark bands on a
//!    light background (Coomassie, silver, colorimetric) are measured as `max − value`, with
//!    `max` the largest value of the plane. `polarity: auto` picks the side of the median the
//!    long tail of the histogram lies on.
//! 2. **Lanes.** Lanes run top to bottom. The column profile is the mean of each column's
//!    brightest tenth above the plane's 5th percentile, smoothed. Its autocorrelation gives the
//!    lane pitch; lanes are the profile's local maxima at least 0.6 pitches apart that rise
//!    above the surrounding valleys by a sixth of the profile's range, each centred on the
//!    signal within half a pitch. Gaps of two or more pitches between found lanes get evenly
//!    spaced lanes (empty lanes are lanes too). `count` keeps the most prominent `count` lanes.
//!    Each lane is 0.7 pitches wide unless `width_px` says otherwise. The lanes span the rows
//!    where their mean signal exceeds a tenth of its 90th percentile (the gel or membrane, not
//!    the ground around it). `lane_contrast` says how far the lanes stand out from the gaps.
//! 3. **Profile.** The mean signal across the middle 80 % of the lane's width, per row.
//! 4. **Background.** A rolling-ball background of the profile (a grey-scale opening by a
//!    window of a tenth of the lane's length, then smoothed, keeping its convex corners), so
//!    smears and uneven blot background are not counted as band.
//! 5. **Bands.** Local maxima of profile − background away from the first and last 3 % of the
//!    lane that stand above the noise (four times the robust standard deviation of the
//!    profile's differences), above `min_share` of the lane's highest band and above 3 % of the
//!    image's strongest band. A band extends to where the signal falls to 5 % of its height or
//!    to the valley between it and the next band; one whose background steps by more than its
//!    height (a membrane edge) is not a band. Bands in the top 10 % of the lanes that three
//!    quarters of the lanes share, above further bands, are the wells and are left out.
//!    **Volume** = the sum of
//!    profile − background over the band's rows × the lane width, in stored units × pixels (the
//!    densitometric volume, as gel-analysis programs report it). A band is `saturated` when any
//!    pixel of it reaches the saturation level: its volume is then a lower bound.
//! 6. **Ladder.** With `ladder` (the lane number and the marker sizes, largest first), the
//!    ladder lane's bands are paired top-down with the sizes and every band gets a size by
//!    linear interpolation of log(size) against migration distance between the neighbouring
//!    marker bands (extrapolated from the end pair outside them).

// x, y, w, h, c, k are the natural names in image arithmetic.
#![allow(clippy::many_single_char_names)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::objects::Polarity;
use super::{Target, round_sig, usage};
use crate::error::{Error, Result};
use crate::reader::{Dataset, PlaneIndex};
use crate::region::Region;

/// Most pixels a lane analysis works on; larger planes are binned to this.
pub const MAX_LANE_PIXELS: u64 = 1 << 22;
/// Most lanes found.
pub const MAX_LANES: usize = 96;
/// Most bands kept per lane.
pub const MAX_BANDS: usize = 40;
/// Profile values returned per lane with `profiles: true`.
pub const LANE_PROFILE_VALUES: usize = 200;
/// Below this lane contrast the lanes are a guess.
const LOW_CONTRAST: f64 = 0.15;
/// Least height of a band, as a share of the image's strongest band.
const GLOBAL_SHARE: f64 = 0.03;
/// Most pixels read for a lane analysis (a plane of 8192 × 8192).
const MAX_READ_PIXELS: u64 = 1 << 26;

/// The marker lane and its sizes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LadderRequest {
    /// Lane number of the marker (1 = leftmost).
    pub lane: u32,
    /// Marker sizes from the top of the lane down (largest first), e.g. kDa or bp.
    pub sizes: Vec<f64>,
    /// Unit of the sizes, e.g. `kDa` or `bp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// How lanes and bands are found. Everything is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanesRequest {
    /// Number of lanes. Default: found from the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Lane width in pixels. Default 0.7 lane pitches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_px: Option<f64>,
    /// Bright bands (fluorescence, chemiluminescence) or dark ones (stains). Default auto.
    #[serde(default)]
    pub polarity: Polarity,
    /// Bands lower than this share of the lane's highest band are dropped. Default 0.05.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_share: Option<f64>,
    /// The marker lane and its sizes; bands then get sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder: Option<LadderRequest>,
    /// Return each lane's profile (200 values from the top down). Default false.
    #[serde(default)]
    pub profiles: bool,
}

/// One band.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Band {
    /// Band number in its lane, from the top.
    pub number: u32,
    /// Row of the band's maximum (full-resolution pixels).
    pub y: f64,
    /// First and last row of the band.
    pub y_range: [f64; 2],
    /// Height of the maximum above the background, in stored units.
    pub height: f64,
    /// Background-subtracted volume: stored units × pixels.
    pub volume: f64,
    /// Share of the lane's band volume.
    pub fraction_of_lane: f64,
    /// Volume relative to the strongest band of the image (1 = the strongest band).
    pub relative: f64,
    /// Size from the ladder, in its unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Pixels of the band at the saturation level: its volume is a lower bound.
    #[serde(skip_serializing_if = "is_zero")]
    pub saturated_px: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One lane.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Lane {
    /// Lane number, 1 = leftmost.
    pub number: u32,
    /// Centre column (full-resolution pixels).
    pub x: f64,
    /// Left and right edges.
    pub x_range: [f64; 2],
    /// True when no band was found (or the lane was placed in a gap between found lanes).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
    /// Background-subtracted volume of the whole lane (stored units × pixels): the total
    /// signal, e.g. for normalising to a total-protein stain.
    pub total_volume: f64,
    /// Sum of the band volumes.
    pub band_volume: f64,
    /// The bands, top first.
    pub bands: Vec<Band>,
    /// The profile from the top down, `profile_step_px` apart (with `profiles: true`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<f64>,
    /// Its background, at the same rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<f64>,
}

/// Lanes and bands of one plane.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Lanes {
    /// Lanes found.
    pub count: u32,
    /// How far the lanes stand out from the gaps between them: 1 − gap signal / lane signal of
    /// the column profile (0 = not at all; below 0.15 the lanes are a guess).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane_contrast: Option<f64>,
    /// `bright` or `dark` bands, as measured.
    pub polarity: String,
    /// Distance between neighbouring lane centres, pixels.
    pub pitch_px: f64,
    /// Lane width, pixels.
    pub width_px: f64,
    /// Rows the lanes span: first and last (full-resolution pixels).
    pub y_range: [f64; 2],
    /// The channel measured.
    pub c: u32,
    /// Full-resolution pixels per analysed pixel (1 unless the plane was binned).
    pub bin: u32,
    /// The value counted as saturated, when the pixel type has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation_level: Option<f64>,
    /// Bands found over all lanes.
    pub band_count: u32,
    /// Unit of band sizes (from the ladder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_unit: Option<String>,
    /// Distance between profile values, pixels (with `profiles: true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_step_px: Option<f64>,
    /// The lanes, left to right.
    pub lanes: Vec<Lane>,
}

/// The signal plane: values where larger = more signal, binned.
struct Signal {
    w: usize,
    h: usize,
    v: Vec<f64>,
    /// Saturated pixels per binned pixel (count of full-resolution pixels at the level).
    sat: Vec<u32>,
    bin: u32,
    x0: u32,
    y0: u32,
}

pub(super) fn lanes(
    ds: &mut dyn Dataset,
    tg: &Target<'_>,
    level: u32,
    region: Option<Region>,
    req: &LanesRequest,
    notes: &mut Vec<String>,
) -> Result<Lanes> {
    let c = tg.channels.first().copied().unwrap_or(0);
    if tg.channels.len() > 1 {
        notes.push(format!(
            "lanes were measured in channel c={c}; select c=N for another"
        ));
    }
    let r = match region {
        Some(r) => {
            r.check_within(tg.level_w, tg.level_h, "region")?;
            r
        }
        None => Region::full(tg.level_w, tg.level_h),
    };
    let n = u64::from(r.width) * u64::from(r.height);
    if n > MAX_READ_PIXELS {
        return Err(Error::unsupported(
            "stats",
            format!("lanes over {} x {} pixels", r.width, r.height),
            "Measure lanes on at most 8192 x 8192 pixels: pass a region around the gel, or a coarser level.",
        ));
    }
    if r.width < 8 || r.height < 8 {
        return Err(usage(
            "lanes need a plane or region of at least 8 x 8 pixels",
        ));
    }
    let idx = PlaneIndex {
        c,
        z: tg.z,
        t: tg.t,
    };
    let p = if r.is_full(tg.level_w, tg.level_h) && level == 0 {
        ds.read_plane(tg.im.index, idx)?
    } else {
        ds.read_region(tg.im.index, idx, level, r)?
    };
    let sat_level = crate::stats::recorded_saturation(tg.im)
        .map(|s| s.0)
        .or_else(|| saturation_of(p.pixel_type));
    let sig = signal(&p, r, sat_level, req.polarity)?;
    let polarity = sig.1;
    let sig = sig.0;
    let mut out = analyse(&sig, req, notes)?;
    out.polarity = polarity.into();
    out.c = c;
    out.saturation_level = sat_level;
    Ok(out)
}

/// Lanes of an 8-bit grey plane `width` pixels wide whose rows are `data` (fuzz targets: the
/// analysis without a reader).
#[doc(hidden)]
pub fn lanes_of_bytes(width: u32, data: &[u8], req: &LanesRequest) -> Result<Lanes> {
    let w = width.max(1);
    let h = u32::try_from(data.len() / w as usize).unwrap_or(u32::MAX);
    let n = w as usize * h as usize;
    let p = crate::pixel::Plane {
        width: w,
        height: h,
        pixel_type: crate::pixel::PixelType::Uint8,
        samples_per_pixel: 1,
        data: data[..n].to_vec(),
    };
    if w < 8 || h < 8 {
        return Err(usage("lanes need a plane of at least 8 x 8 pixels"));
    }
    let (sig, polarity) = signal(&p, Region::full(w, h), Some(255.0), req.polarity)?;
    let mut out = analyse(&sig, req, &mut Vec::new())?;
    out.polarity = polarity.into();
    Ok(out)
}

fn saturation_of(p: crate::pixel::PixelType) -> Option<f64> {
    use crate::pixel::PixelType;
    match p {
        PixelType::Uint8 => Some(255.0),
        PixelType::Uint16 => Some(65535.0),
        _ => None,
    }
}

/// The plane as signal (larger = more), binned to at most [`MAX_LANE_PIXELS`].
fn signal(
    p: &crate::pixel::Plane,
    r: Region,
    sat_level: Option<f64>,
    polarity: Polarity,
) -> Result<(Signal, &'static str)> {
    let spp = p.samples_per_pixel.max(1) as usize;
    let bps = p.pixel_type.bytes_per_sample();
    let (w, h) = (p.width as usize, p.height as usize);
    let need = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(spp))
        .and_then(|n| n.checked_mul(bps))
        .unwrap_or(usize::MAX);
    if p.data.len() < need || w == 0 || h == 0 {
        return Err(Error::corrupt(
            "stats",
            format!("plane holds {} bytes, {w} x {h} needs {need}", p.data.len()),
        ));
    }
    let n = (w * h) as u64;
    let mut bin = 1usize;
    while n / (bin * bin) as u64 > MAX_LANE_PIXELS {
        bin += 1;
    }
    let (bw, bh) = (w.div_ceil(bin), h.div_ceil(bin));
    let mut sum = vec![0.0f64; bw * bh];
    let mut cnt = vec![0u32; bw * bh];
    let mut sat = vec![0u32; bw * bh];
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let row_bytes = w * spp * bps;
    for y in 0..h {
        let row = &p.data[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..w {
            let px = &row[x * spp * bps..(x + 1) * spp * bps];
            // RGB: the mean of the components
            let mut v = 0.0;
            let mut s = false;
            for k in 0..spp {
                let a = p.pixel_type.sample_f64(&px[k * bps..(k + 1) * bps]);
                if sat_level.is_some_and(|l| a >= l) {
                    s = true;
                }
                v += a;
            }
            v /= spp as f64;
            if !v.is_finite() {
                continue;
            }
            lo = lo.min(v);
            hi = hi.max(v);
            let i = (y / bin) * bw + x / bin;
            sum[i] += v;
            cnt[i] += 1;
            if s {
                sat[i] += 1;
            }
        }
    }
    if !(lo.is_finite() && hi.is_finite()) || hi <= lo {
        return Err(usage(
            "the plane is flat (every pixel has the same value): there are no lanes to find",
        ));
    }
    let mut v: Vec<f64> = sum
        .iter()
        .zip(&cnt)
        .map(|(s, c)| if *c > 0 { s / f64::from(*c) } else { lo })
        .collect();
    let dark = match polarity {
        Polarity::Bright => false,
        Polarity::Dark => true,
        Polarity::Auto => {
            let mut s = v.clone();
            s.sort_by(f64::total_cmp);
            let q = |f: f64| s[((s.len() - 1) as f64 * f) as usize];
            let (p1, med, p99) = (q(0.005), q(0.5), q(0.995));
            // the tail of the histogram is where the bands are
            med - p1 > p99 - med
        }
    };
    if dark {
        for x in &mut v {
            *x = hi - *x;
        }
        // a dark band never reaches the bright saturation level
        sat.fill(0);
    }
    Ok((
        Signal {
            w: bw,
            h: bh,
            v,
            sat,
            bin: bin as u32,
            x0: r.x,
            y0: r.y,
        },
        if dark { "dark" } else { "bright" },
    ))
}

fn percentile(v: &[f64], f: f64) -> f64 {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * f.clamp(0.0, 1.0)).round() as usize]
}

/// Gaussian smoothing with reflected ends.
fn smooth(v: &[f64], sigma: f64) -> Vec<f64> {
    if sigma < 0.3 || v.len() < 3 {
        return v.to_vec();
    }
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f64> = (-r..=r)
        .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f64 = k.iter().sum();
    let n = v.len() as isize;
    (0..n)
        .map(|i| {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let mut t = i + j as isize - r;
                if t < 0 {
                    t = -t - 1;
                }
                if t >= n {
                    t = 2 * n - t - 1;
                }
                let t = t.clamp(0, n - 1) as usize;
                s += kv * v[t];
            }
            s / ks
        })
        .collect()
}

/// Running minimum then maximum over `win` samples: a grey-scale opening (rolling ball).
fn opening(v: &[f64], win: usize) -> Vec<f64> {
    let half = win / 2;
    let n = v.len();
    let run = |src: &[f64], max: bool| -> Vec<f64> {
        (0..n)
            .map(|i| {
                let a = i.saturating_sub(half);
                let b = (i + half + 1).min(n);
                let it = src[a..b].iter().copied();
                if max {
                    it.fold(f64::NEG_INFINITY, f64::max)
                } else {
                    it.fold(f64::INFINITY, f64::min)
                }
            })
            .collect()
    };
    let e = run(v, false);
    run(&e, true)
}

/// Local maxima of `v` with their prominence (height above the higher of the two valleys
/// that separate them from higher ground).
fn maxima(v: &[f64]) -> Vec<(usize, f64)> {
    let n = v.len();
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < n {
        if v[i] > v[i - 1] {
            // plateau
            let mut j = i;
            while j + 1 < n && v[j + 1].total_cmp(&v[i]).is_eq() {
                j += 1;
            }
            if j + 1 < n && v[j + 1] < v[i] {
                let peak = usize::midpoint(i, j);
                let h = v[peak];
                let mut lmin = h;
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    if v[k] > h {
                        break;
                    }
                    lmin = lmin.min(v[k]);
                }
                let mut rmin = h;
                let mut k = j;
                while k + 1 < n {
                    k += 1;
                    if v[k] > h {
                        break;
                    }
                    rmin = rmin.min(v[k]);
                }
                out.push((peak, h - lmin.max(rmin)));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The lag of the first clear autocorrelation peak of `v` between `lo` and `hi` samples.
fn pitch(v: &[f64], lo: usize, hi: usize) -> Option<f64> {
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let d: Vec<f64> = v.iter().map(|x| x - mean).collect();
    let var: f64 = d.iter().map(|x| x * x).sum();
    if var <= 0.0 {
        return None;
    }
    let hi = hi.min(n.saturating_sub(2));
    if lo >= hi {
        return None;
    }
    let ac: Vec<f64> = (0..=hi)
        .map(|l| {
            d[..n - l]
                .iter()
                .zip(&d[l..])
                .map(|(a, b)| a * b)
                .sum::<f64>()
                / var
        })
        .collect();
    let peaks = maxima(&ac);
    let best = peaks
        .iter()
        .filter(|(l, _)| *l >= lo && *l <= hi && ac[*l] > 0.05)
        .max_by(|a, b| ac[a.0].total_cmp(&ac[b.0]))?;
    // the first peak at least half as strong as the best one (a multiple of the pitch is as
    // strong as the pitch on a regular gel)
    let first = peaks
        .iter()
        .filter(|(l, _)| *l >= lo && *l <= hi && ac[*l] >= 0.6 * ac[best.0])
        .min_by_key(|p| p.0)
        .copied()
        .unwrap_or(*best);
    // sub-sample position by a parabola through the peak and its neighbours
    let l = first.0;
    if l == 0 || l + 1 >= ac.len() {
        return Some(l as f64);
    }
    let (a, b, c) = (ac[l - 1], ac[l], ac[l + 1]);
    let den = a - 2.0 * b + c;
    Some(if den.abs() > 1e-12 {
        l as f64 + 0.5 * (a - c) / den
    } else {
        l as f64
    })
}

fn analyse(s: &Signal, req: &LanesRequest, notes: &mut Vec<String>) -> Result<Lanes> {
    let (w, h) = (s.w, s.h);
    let bin = f64::from(s.bin);
    let floor = percentile(&s.v, 0.05);
    // column profile: the mean of each column's brightest tenth above the floor, so a lane of
    // a few thin bands stands out as well as a lane full of them
    let top = (h / 10).max(1);
    let mut colbuf = vec![0.0; h];
    let col: Vec<f64> = (0..w)
        .map(|x| {
            for (y, c) in colbuf.iter_mut().enumerate() {
                *c = (s.v[y * w + x] - floor).max(0.0);
            }
            colbuf.select_nth_unstable_by(h - top, f64::total_cmp);
            colbuf[h - top..].iter().sum::<f64>() / top as f64
        })
        .collect();
    let colsm = smooth(&col, (w as f64 / 300.0).max(1.0));
    let range = colsm.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - colsm.iter().copied().fold(f64::INFINITY, f64::min);
    if range.is_nan() || range <= 0.0 {
        return Err(usage(
            "no lanes found: the columns of the plane all have the same signal",
        ));
    }
    // pitch
    let user_width = req
        .width_px
        .filter(|x| x.is_finite() && *x >= 1.0)
        .map(|x| x / bin);
    let mut p = match req.count.filter(|c| *c > 0) {
        Some(n) => None.or_else(|| {
            // from the count: the signal's span over n
            let thr = colsm.iter().copied().fold(f64::INFINITY, f64::min) + range * 0.15;
            let first = colsm.iter().position(|v| *v > thr)?;
            let last = colsm.iter().rposition(|v| *v > thr)?;
            let span = (last - first) as f64;
            Some(if n > 1 {
                span / (f64::from(n) - 1.0 + 0.7)
            } else {
                span.max(4.0)
            })
        }),
        None => pitch(&colsm, (w / 60).max(3), w / 2),
    };
    if let Some(uw) = user_width {
        p = Some(p.map_or(uw / 0.7, |p| p.max(uw)));
    }
    let pitch_px = p.unwrap_or((w as f64 / 10.0).max(4.0));
    // lanes: maxima at least 0.6 pitches apart, prominent enough
    let mut cand: Vec<(usize, f64)> = maxima(&colsm)
        .into_iter()
        .filter(|(_, pr)| *pr >= range / 6.0)
        .collect();
    cand.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<usize> = Vec::new();
    for (x, _) in &cand {
        if kept
            .iter()
            .all(|k| ((*k as f64) - (*x as f64)).abs() >= 0.6 * pitch_px)
        {
            kept.push(*x);
        }
        if kept.len() >= MAX_LANES {
            break;
        }
    }
    if let Some(n) = req.count.filter(|c| *c > 0) {
        kept.truncate(n as usize);
    }
    kept.sort_unstable();
    let mut centers: Vec<(f64, bool)> = kept.iter().map(|x| (*x as f64, false)).collect();
    // fill gaps of two or more pitches
    if centers.len() >= 2 && req.count.is_none_or(|n| (centers.len() as u32) < n) {
        let mut filled = Vec::new();
        for win in centers.windows(2) {
            filled.push(win[0]);
            let gap = win[1].0 - win[0].0;
            let k = (gap / pitch_px).round();
            if k >= 2.0 && (gap / k - pitch_px).abs() < 0.25 * pitch_px {
                for j in 1..k as usize {
                    filled.push((win[0].0 + gap * j as f64 / k, true));
                }
            }
        }
        filled.push(*centers.last().unwrap_or(&(0.0, false)));
        if req.count.is_none_or(|n| filled.len() as u32 <= n) {
            centers = filled;
        }
    }
    if centers.is_empty() {
        return Err(Error::unsupported(
            "stats",
            "lanes in this image (no column stands out)",
            "Pass count (the number of lanes), or a region around the gel; check the picture with preview marks=lanes.",
        ));
    }
    if let Some(n) = req.count
        && (centers.len() as u32) < n
    {
        notes.push(format!(
            "found {} of the {n} lanes asked for; the others do not stand out from the background",
            centers.len()
        ));
    }
    let lane_w = user_width.unwrap_or(0.7 * pitch_px).clamp(1.0, w as f64);
    // centre each found lane on the signal within half a pitch of it
    for (cx, filled) in &mut centers {
        if *filled {
            continue;
        }
        let a = (*cx - pitch_px / 2.0).max(0.0) as usize;
        let b = ((*cx + pitch_px / 2.0) as usize + 1).min(w);
        let lo = col[a..b].iter().copied().fold(f64::INFINITY, f64::min);
        let (mut sw, mut sx) = (0.0, 0.0);
        for (x, v) in col.iter().enumerate().take(b).skip(a) {
            let wgt = (v - lo).max(0.0);
            sw += wgt;
            sx += wgt * x as f64;
        }
        if sw > 0.0 {
            *cx = sx / sw;
        }
    }
    // rows the gel spans: where the lanes' mean signal rises above a tenth of its upper level
    // (a membrane or gel on a dark ground ends there)
    let (ya, yb) = {
        let cols: Vec<usize> = centers
            .iter()
            .flat_map(|(cx, _)| {
                let a = (cx - lane_w / 2.0).round().max(0.0) as usize;
                let b = ((cx + lane_w / 2.0).round() as usize).min(w);
                a..b
            })
            .collect();
        let rows: Vec<f64> = (0..h)
            .map(|y| {
                cols.iter()
                    .map(|x| (s.v[y * w + x] - floor).max(0.0))
                    .sum::<f64>()
                    / cols.len().max(1) as f64
            })
            .collect();
        let rs = smooth(&rows, (h as f64 / 200.0).max(1.0));
        let cut = percentile(&rs, 0.9) * 0.1;
        let a = rs.iter().position(|v| *v > cut).unwrap_or(0);
        let b = rs.iter().rposition(|v| *v > cut).map_or(h, |b| b + 1);
        if b > a && b - a >= h / 8 {
            (a, b)
        } else {
            (0, h)
        }
    };
    let min_share = req
        .min_share
        .filter(|x| x.is_finite() && *x >= 0.0)
        .unwrap_or(0.05);
    // how far the lanes stand out from the gaps between them
    let contrast = {
        let at = |x: f64| colsm[(x.round().max(0.0) as usize).min(w - 1)];
        let lanes_mean = centers.iter().map(|(c, _)| at(*c)).sum::<f64>() / centers.len() as f64;
        let gaps: Vec<f64> = centers
            .windows(2)
            .map(|p| at(f64::midpoint(p[0].0, p[1].0)))
            .collect();
        if gaps.is_empty() || lanes_mean <= 0.0 {
            None
        } else {
            let g = gaps.iter().sum::<f64>() / gaps.len() as f64;
            Some(((lanes_mean - g) / lanes_mean).clamp(0.0, 1.0))
        }
    };
    if contrast.is_some_and(|c| c < LOW_CONTRAST) {
        notes.push(format!(
            "the lanes barely stand out from the gaps between them (lane_contrast {:.2}): look at the picture (preview marks=lanes) and pass count, width_px or a region around the lanes",
            contrast.unwrap_or(0.0)
        ));
    }
    // each lane's profile: the mean across the middle 80 % of its width
    let mut profs = Vec::new();
    for (cx, filled) in &centers {
        let xa = ((cx - lane_w / 2.0).round().max(0.0)) as usize;
        let xb = ((cx + lane_w / 2.0).round() as usize + 1)
            .min(w)
            .max(xa + 1);
        let inset = ((xb - xa) as f64 * 0.1).floor() as usize;
        let (ia, ib) = (xa + inset, (xb - inset).max(xa + inset + 1));
        let mut prof = vec![0.0; yb - ya];
        let mut satrow = vec![0u64; yb - ya];
        for y in ya..yb {
            let row = &s.v[y * w..(y + 1) * w];
            prof[y - ya] = row[ia..ib].iter().sum::<f64>() / (ib - ia) as f64;
            satrow[y - ya] = s.sat[y * w + xa..y * w + xb]
                .iter()
                .map(|v| u64::from(*v))
                .sum();
        }
        profs.push((*cx, xa, xb, *filled, prof, satrow));
    }
    // a band also stands at least 3 % of the image's strongest band above its background, so an
    // empty lane's noise is not read as bands
    let tops: Vec<f64> = profs
        .iter()
        .map(|(_, xa, xb, _, prof, satrow)| {
            lane_bands(prof, satrow, (xb - xa) as f64, min_share, h, 0.0)
                .bands
                .iter()
                .map(|b| b.height)
                .fold(0.0, f64::max)
        })
        .collect();
    let global = tops.iter().copied().fold(0.0, f64::max) * GLOBAL_SHARE;
    let mut lanes = Vec::new();
    for (i, (cx, xa, xb, filled, prof, satrow)) in profs.into_iter().enumerate() {
        let lane = lane_bands(&prof, &satrow, (xb - xa) as f64, min_share, h, global);
        lanes.push((i, cx, xa, xb, filled, prof, lane));
    }
    // to full-resolution pixels
    let scale = bin;
    let (ox, oy) = (f64::from(s.x0), f64::from(s.y0));
    let px_area = bin * bin;
    let mut out_lanes: Vec<Lane> = Vec::new();
    for (i, cx, xa, xb, _filled, prof, lb) in lanes {
        let bands: Vec<Band> = lb
            .bands
            .iter()
            .enumerate()
            .map(|(k, b)| Band {
                number: k as u32 + 1,
                y: tenth(oy + (b.y + ya as f64 + 0.5) * scale),
                y_range: [
                    oy + (b.y0 + ya) as f64 * scale,
                    oy + (b.y1 + ya + 1) as f64 * scale,
                ],
                height: round_sig(b.height),
                volume: b.volume * px_area,
                fraction_of_lane: 0.0,
                relative: 0.0,
                size: None,
                saturated_px: b.sat,
            })
            .collect();
        let band_volume: f64 = bands.iter().map(|b| b.volume).sum();
        let (profile, background) = if req.profiles {
            (
                super::resample(&prof, LANE_PROFILE_VALUES.min(prof.len()))
                    .iter()
                    .map(|v| round_sig(*v))
                    .collect(),
                super::resample(&lb.background, LANE_PROFILE_VALUES.min(prof.len()))
                    .iter()
                    .map(|v| round_sig(*v))
                    .collect(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        out_lanes.push(Lane {
            number: i as u32 + 1,
            x: tenth(ox + (cx + 0.5) * scale),
            x_range: [ox + xa as f64 * scale, ox + xb as f64 * scale],
            empty: bands.is_empty(),
            total_volume: lb.total * px_area,
            band_volume,
            bands,
            profile,
            background,
        });
    }
    drop_wells(
        &mut out_lanes,
        [oy + ya as f64 * scale, oy + yb as f64 * scale],
        notes,
    );
    let max_vol = out_lanes
        .iter()
        .flat_map(|l| l.bands.iter().map(|b| b.volume))
        .fold(0.0, f64::max);
    for l in &mut out_lanes {
        let bv = l.band_volume;
        for b in &mut l.bands {
            b.fraction_of_lane = if bv > 0.0 {
                round_sig(b.volume / bv)
            } else {
                0.0
            };
            b.relative = if max_vol > 0.0 {
                round_sig(b.volume / max_vol)
            } else {
                0.0
            };
            b.volume = round_sig(b.volume);
        }
        l.band_volume = round_sig(l.band_volume);
        l.total_volume = round_sig(l.total_volume);
    }
    let mut size_unit = None;
    if let Some(ld) = &req.ladder {
        size_unit = ld.unit.clone();
        apply_ladder(&mut out_lanes, ld, notes)?;
    }
    let sat_bands = out_lanes
        .iter()
        .flat_map(|l| &l.bands)
        .filter(|b| b.saturated_px > 0)
        .count();
    if sat_bands > 0 {
        notes.push(format!(
            "{sat_bands} band(s) hold saturated pixels: their volumes are lower bounds (use a shorter exposure for quantification)"
        ));
    }
    let profiles = req.profiles;
    Ok(Lanes {
        count: out_lanes.len() as u32,
        lane_contrast: contrast.map(round_sig),
        polarity: String::new(),
        pitch_px: round_sig(pitch_px * scale),
        width_px: round_sig(lane_w * scale),
        y_range: [oy + ya as f64 * scale, oy + yb as f64 * scale],
        c: 0,
        bin: s.bin,
        saturation_level: None,
        band_count: out_lanes.iter().map(|l| l.bands.len() as u32).sum(),
        size_unit,
        profile_step_px: profiles.then(|| {
            let n = LANE_PROFILE_VALUES.min(yb - ya).max(2);
            round_sig((yb - ya - 1) as f64 * scale / (n - 1) as f64)
        }),
        lanes: out_lanes,
    })
}

struct RawBand {
    y: f64,
    y0: usize,
    y1: usize,
    height: f64,
    volume: f64,
    sat: u64,
}

struct LaneBands {
    bands: Vec<RawBand>,
    background: Vec<f64>,
    total: f64,
}

/// Bands of one lane profile (binned pixels).
fn lane_bands(
    prof: &[f64],
    satrow: &[u64],
    width: f64,
    min_share: f64,
    h: usize,
    floor: f64,
) -> LaneBands {
    let n = prof.len();
    let sm = smooth(prof, (h as f64 / 400.0).max(0.8));
    let win = (n / 10).max(5);
    let op = opening(&sm, win);
    // smoothing rounds the opening's corners: keep its convex corners (the ends of a membrane
    // on a dark ground are not bands) and keep it under the profile
    let bg: Vec<f64> = smooth(&op, (win as f64 / 4.0).max(1.0))
        .iter()
        .zip(&op)
        .zip(&sm)
        .map(|((b, o), v)| b.max(*o).min(*v))
        .collect();
    let d: Vec<f64> = sm.iter().zip(&bg).map(|(v, b)| (v - b).max(0.0)).collect();
    let total: f64 = prof
        .iter()
        .zip(&bg)
        .map(|(v, b)| (v - b).max(0.0))
        .sum::<f64>()
        * width;
    // noise: robust spread of the raw profile's differences
    let mut diffs: Vec<f64> = prof.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    diffs.sort_by(f64::total_cmp);
    let mad = diffs.get(diffs.len() / 2).copied().unwrap_or(0.0);
    let noise = mad * 1.4826 / std::f64::consts::SQRT_2;
    let top = d.iter().copied().fold(0.0, f64::max);
    let thr = (4.0 * noise)
        .max(min_share * top)
        .max(floor)
        .max(f64::MIN_POSITIVE);
    let mut peaks: Vec<(usize, f64)> = maxima(&d)
        .into_iter()
        // the first and last 3 % of the lane are its ends (a well, the edge of the membrane)
        .filter(|(i, pr)| {
            d[*i] >= thr && *pr >= thr * 0.5 && *i * 100 >= n * 3 && *i * 100 <= n * 97
        })
        .collect();
    peaks.sort_by(|a, b| d[b.0].total_cmp(&d[a.0]));
    peaks.truncate(MAX_BANDS);
    peaks.sort_by_key(|p| p.0);
    let mut bands = Vec::new();
    for (k, (i, _)) in peaks.iter().enumerate() {
        let hgt = d[*i];
        let stop = hgt * 0.05;
        let left_lim = if k > 0 { peaks[k - 1].0 } else { 0 };
        let right_lim = peaks.get(k + 1).map_or(n - 1, |p| p.0);
        // walk out to 5 % of the height, or to the valley before the neighbour
        let mut a = *i;
        while a > left_lim && d[a - 1] > stop && !(d[a - 1] > d[a] && a < *i) {
            a -= 1;
        }
        if a > left_lim || k == 0 {
            // ok
        }
        let valley = |lo: usize, hi: usize| -> usize {
            (lo..=hi)
                .min_by(|x, y| d[*x].total_cmp(&d[*y]))
                .unwrap_or(lo)
        };
        if k > 0 && a <= left_lim {
            a = valley(left_lim, *i) + 1;
        }
        let mut b = *i;
        while b < right_lim && d[b + 1] > stop && !(d[b + 1] > d[b] && b > *i) {
            b += 1;
        }
        if k + 1 < peaks.len() && b >= right_lim {
            b = valley(*i, right_lim);
        }
        let a = a.min(*i);
        let b = b.max(*i);
        let volume: f64 = (a..=b).map(|y| (prof[y] - bg[y]).max(0.0)).sum::<f64>() * width;
        // sub-pixel apex
        let y = if *i > 0 && *i + 1 < n {
            let (l, c, r) = (d[i - 1], d[*i], d[i + 1]);
            let den = l - 2.0 * c + r;
            if den.abs() > 1e-12 {
                *i as f64 + (0.5 * (l - r) / den).clamp(-0.5, 0.5)
            } else {
                *i as f64
            }
        } else {
            *i as f64
        };
        // a step of the background as large as the band (the bright rim of a membrane against
        // the dark ground beside it) is an edge, not a band
        let (ba, bb) = (bg[a], bg[b]);
        if ba.min(bb) < 0.25 * ba.max(bb) && ba.max(bb) - ba.min(bb) > hgt {
            continue;
        }
        bands.push(RawBand {
            y,
            y0: a,
            y1: b,
            height: hgt,
            volume,
            sat: satrow[a..=b].iter().sum(),
        });
    }
    LaneBands {
        bands,
        background: bg,
        total,
    }
}

/// Leave out the wells: bands in the top 10 % of the lanes that at least three quarters of the
/// lanes (three or more) have at the same row, within 1.5 % of the lanes' length, when half of
/// those lanes also have bands further down. Sample wells fill with dye or glow under the
/// imager and read as a band at the top of every lane, above the samples' own bands.
fn drop_wells(lanes: &mut [Lane], y_range: [f64; 2], notes: &mut Vec<String>) {
    let len = (y_range[1] - y_range[0]).max(1.0);
    let top = y_range[0] + 0.10 * len;
    let tol = 0.015 * len;
    let n = lanes.len();
    if n < 3 {
        return;
    }
    let candidates: Vec<f64> = lanes
        .iter()
        .flat_map(|l| l.bands.iter().map(|b| b.y))
        .filter(|y| *y <= top)
        .collect();
    let mut wells: Vec<f64> = Vec::new();
    for y in candidates {
        let with: Vec<&Lane> = lanes
            .iter()
            .filter(|l| l.bands.iter().any(|b| (b.y - y).abs() <= tol))
            .collect();
        let below = with
            .iter()
            .filter(|l| l.bands.iter().any(|b| b.y > y + tol))
            .count();
        let lanes_with = with.len();
        if lanes_with * 4 >= n * 3
            && below * 2 >= lanes_with
            && !wells.iter().any(|w| (w - y).abs() <= tol)
        {
            wells.push(y);
        }
    }
    if wells.is_empty() {
        return;
    }
    let mut dropped = 0;
    for l in lanes.iter_mut() {
        let before = l.bands.len();
        l.bands
            .retain(|b| !wells.iter().any(|w| (b.y - w).abs() <= tol));
        dropped += before - l.bands.len();
        for (k, b) in l.bands.iter_mut().enumerate() {
            b.number = k as u32 + 1;
        }
        l.band_volume = l.bands.iter().map(|b| b.volume).sum();
        l.empty = l.bands.is_empty();
    }
    wells.sort_by(f64::total_cmp);
    notes.push(format!(
        "{dropped} band(s) at the top of the lanes, at row(s) {} that nearly every lane shares, were taken for the wells and left out (pass a region below them to be sure)",
        wells
            .iter()
            .map(|w| format!("{w:.0}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// A pixel position to a tenth of a pixel.
fn tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn apply_ladder(lanes: &mut [Lane], ld: &LadderRequest, notes: &mut Vec<String>) -> Result<()> {
    let Some(lane) = lanes.iter().find(|l| l.number == ld.lane) else {
        return Err(usage(format!(
            "ladder lane {} does not exist ({} lanes found; 1 = leftmost)",
            ld.lane,
            lanes.len()
        )));
    };
    let sizes: Vec<f64> = ld
        .sizes
        .iter()
        .copied()
        .filter(|s| s.is_finite() && *s > 0.0)
        .collect();
    if sizes.len() < 2 || sizes.windows(2).any(|w| w[1] >= w[0]) {
        return Err(usage(
            "ladder sizes must be two or more positive numbers, largest first (the order they run in, top down)",
        ));
    }
    let ys: Vec<f64> = lane.bands.iter().map(|b| b.y).collect();
    if ys.len() < 2 {
        return Err(usage(format!(
            "the ladder lane {} has {} band(s): at least two are needed to size bands",
            ld.lane,
            ys.len()
        )));
    }
    let k = ys.len().min(sizes.len());
    if ys.len() != sizes.len() {
        notes.push(format!(
            "the ladder lane has {} bands and {} sizes were given: the top {k} bands were paired with the first {k} sizes",
            ys.len(),
            sizes.len()
        ));
    }
    let pts: Vec<(f64, f64)> = ys[..k]
        .iter()
        .zip(&sizes[..k])
        .map(|(y, s)| (*y, s.ln()))
        .collect();
    for l in lanes.iter_mut() {
        for b in &mut l.bands {
            let j = pts
                .iter()
                .position(|p| p.0 > b.y)
                .unwrap_or(k)
                .clamp(1, k - 1);
            let (a, c) = (pts[j - 1], pts[j]);
            if (c.0 - a.0).abs() < 1e-9 {
                continue;
            }
            let t = (b.y - a.0) / (c.0 - a.0);
            b.size = Some(round_sig((a.1 + t * (c.1 - a.1)).exp()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic gel: `n` lanes of `bands` Gaussian bands on a sloping background.
    fn gel(w: usize, h: usize, lanes: &[f64], bands: &[(f64, f64)]) -> Signal {
        let mut v = vec![0.0; w * h];
        let lw = 0.6 * (lanes[1] - lanes[0]);
        for y in 0..h {
            for x in 0..w {
                let mut s = 100.0 + y as f64 * 0.2;
                for (li, cx) in lanes.iter().enumerate() {
                    if (x as f64 - cx).abs() <= lw / 2.0 {
                        for (by, amp) in bands {
                            let a = amp * (li as f64 + 1.0);
                            s += a * (-(y as f64 - by).powi(2) / 18.0).exp();
                        }
                    }
                }
                v[y * w + x] = s;
            }
        }
        Signal {
            w,
            h,
            v,
            sat: vec![0; w * h],
            bin: 1,
            x0: 0,
            y0: 0,
        }
    }

    #[test]
    fn finds_lanes_and_bands_of_a_synthetic_gel() {
        let centres = [40.0, 90.0, 140.0, 190.0, 240.0];
        let s = gel(280, 300, &centres, &[(80.0, 500.0), (180.0, 250.0)]);
        let mut notes = Vec::new();
        let out = analyse(&s, &LanesRequest::default(), &mut notes).unwrap();
        assert_eq!(out.count, 5, "{out:#?}");
        for (l, c) in out.lanes.iter().zip(centres) {
            assert!((l.x - 0.5 - c).abs() < 3.0, "lane at {} not {c}", l.x);
            assert_eq!(l.bands.len(), 2, "{l:#?}");
            assert!((l.bands[0].y - 80.5).abs() < 1.5);
            assert!((l.bands[1].y - 180.5).abs() < 1.5);
        }
        // volume grows with the lane number (amplitude × lane)
        let v: Vec<f64> = out.lanes.iter().map(|l| l.bands[0].volume).collect();
        for k in 1..5 {
            let ratio = v[k] / v[0];
            assert!((ratio - (k as f64 + 1.0)).abs() < 0.1, "{v:?}");
        }
        assert!((out.lanes[4].bands[0].relative - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_lane_between_two_is_kept() {
        let centres = [40.0, 90.0, 140.0, 190.0, 240.0];
        let mut s = gel(280, 300, &centres, &[(100.0, 400.0)]);
        // blank lane 3
        for y in 0..300 {
            for x in 120..160 {
                s.v[y * 280 + x] = 100.0 + y as f64 * 0.2;
            }
        }
        let out = analyse(&s, &LanesRequest::default(), &mut Vec::new()).unwrap();
        assert_eq!(out.count, 5, "{out:#?}");
        assert!(out.lanes[2].empty);
        assert!(out.lanes[2].bands.is_empty());
    }

    #[test]
    fn ladder_sizes_bands() {
        let centres = [40.0, 90.0, 140.0];
        let s = gel(
            180,
            300,
            &centres,
            &[(50.0, 300.0), (150.0, 300.0), (250.0, 300.0)],
        );
        let req = LanesRequest {
            ladder: Some(LadderRequest {
                lane: 1,
                sizes: vec![100.0, 50.0, 25.0],
                unit: Some("kDa".into()),
            }),
            ..LanesRequest::default()
        };
        let out = analyse(&s, &req, &mut Vec::new()).unwrap();
        let b = &out.lanes[2].bands;
        assert!((b[1].size.unwrap() - 50.0).abs() < 1.0, "{b:#?}");
    }

    #[test]
    fn wells_are_not_bands() {
        let centres = [40.0, 90.0, 140.0, 190.0, 240.0];
        // a bright well at row 15 of every lane, sample bands at 120 and 200
        let s = gel(
            280,
            300,
            &centres,
            &[(15.0, 900.0), (120.0, 300.0), (200.0, 200.0)],
        );
        let mut notes = Vec::new();
        let out = analyse(&s, &LanesRequest::default(), &mut notes).unwrap();
        for l in &out.lanes {
            assert_eq!(l.bands.len(), 2, "{l:#?}");
            assert!(l.bands[0].y > 100.0);
        }
        assert!(notes.iter().any(|n| n.contains("wells")), "{notes:?}");
    }

    #[test]
    fn a_flat_signal_is_a_usage_error() {
        let s = Signal {
            w: 20,
            h: 20,
            v: vec![1.0; 400],
            sat: vec![0; 400],
            bin: 1,
            x0: 0,
            y0: 0,
        };
        assert!(analyse(&s, &LanesRequest::default(), &mut Vec::new()).is_err());
    }
}
