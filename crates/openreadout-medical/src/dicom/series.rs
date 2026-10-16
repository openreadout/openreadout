//! From files and frames to images: per-frame geometry and value transforms (classic headers
//! and multi-frame functional groups, PS3.3 C.7.6.2 and C.7.6.16), grouping frames into
//! images, and ordering them along Z (the slice normal) and T.

use std::collections::BTreeMap;

use openreadout_core::PixelType;

use super::element::{Lookup, functional};
use super::file::DicomFile;
use super::parse::Item;
use super::pixels::PixelDesc;
use super::tags as t;
use super::values::{Modality, Palette, output_type};
use crate::util::{cross, dot, median};

/// What one frame of one file is.
#[derive(Debug, Clone)]
pub(crate) struct FrameRec {
    pub(crate) file: usize,
    pub(crate) frame: u32,
    /// ImagePositionPatient of the first pixel, mm.
    pub(crate) position: Option<[f64; 3]>,
    /// ImageOrientationPatient: row then column direction cosines.
    pub(crate) orientation: Option<[f64; 6]>,
    /// PixelSpacing: (between rows, between columns) in mm.
    pub(crate) spacing: Option<[f64; 2]>,
    pub(crate) thickness: Option<f64>,
    pub(crate) modality: Modality,
    pub(crate) instance: Option<f64>,
    /// TemporalPositionIndex or TemporalPositionIdentifier.
    pub(crate) temporal: Option<f64>,
    pub(crate) trigger_ms: Option<f64>,
    /// Acquisition time of day in seconds.
    pub(crate) time_s: Option<f64>,
    pub(crate) slice_location: Option<f64>,
    pub(crate) window: Option<(f64, f64)>,
    /// Segmentation frames: the segment number (ReferencedSegmentNumber).
    pub(crate) segment: Option<u32>,
}

/// Seconds since midnight from a DICOM TM value (`HHMMSS.FFFFFF`, also `HH:MM:SS`).
pub(crate) fn tm_seconds(s: &str) -> Option<f64> {
    let s = s.trim().replace(':', "");
    if s.len() < 2 || !s.as_bytes()[..2].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let h: f64 = s.get(0..2)?.parse().ok()?;
    let m: f64 = s.get(2..4).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let sec: f64 = s
        .get(4..)
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn arr3(v: &[f64]) -> Option<[f64; 3]> {
    (v.len() >= 3 && v[..3].iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
}

fn arr6(v: &[f64]) -> Option<[f64; 6]> {
    (v.len() >= 6 && v[..6].iter().all(|x| x.is_finite()))
        .then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

fn spacing2(v: &[f64]) -> Option<[f64; 2]> {
    (v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0 && v[0].is_finite() && v[1].is_finite())
        .then(|| [v[0], v[1]])
}

/// The unit normal of an orientation (row × column).
pub(crate) fn normal(o: &[f64; 6]) -> [f64; 3] {
    let n = cross([o[0], o[1], o[2]], [o[3], o[4], o[5]]);
    let len = dot(n, n).sqrt();
    if len > 0.0 {
        n.map(|v| v / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Every frame of `file` (index `fi`) with its geometry and transform.
pub(crate) fn frames_of(file: &DicomFile, fi: usize, desc: &PixelDesc) -> Vec<FrameRec> {
    let ds = &file.ds;
    let shared = ds.first(t::SHARED_FUNCTIONAL_GROUPS_SEQUENCE);
    let per_frame = ds.items(t::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE);
    let top_pos = arr3(&ds.numbers(t::IMAGE_POSITION_PATIENT));
    let top_ori = arr6(&ds.numbers(t::IMAGE_ORIENTATION_PATIENT));
    let top_spacing = spacing2(&ds.numbers(t::PIXEL_SPACING));
    let top_modality = Modality::of(file, ds);
    let top_window = window_of(ds);
    let instance = ds.number(t::INSTANCE_NUMBER);
    let time_s = ds
        .text(t::ACQUISITION_TIME)
        .or_else(|| ds.text(t::CONTENT_TIME))
        .and_then(|s| tm_seconds(&s));
    // classic multi-frame files that step through space (RT dose, NM tomography)
    let offsets = ds.numbers(t::GRID_FRAME_OFFSET_VECTOR);
    let frame_time = ds.number(t::FRAME_TIME);
    let mut out = Vec::with_capacity(desc.frames as usize);
    for f in 0..desc.frames {
        let pf = per_frame.get(f as usize);
        let pos = functional(pf, shared, t::PLANE_POSITION_SEQUENCE)
            .and_then(|i| arr3(&i.numbers(t::IMAGE_POSITION_PATIENT)));
        let ori = functional(pf, shared, t::PLANE_ORIENTATION_SEQUENCE)
            .and_then(|i| arr6(&i.numbers(t::IMAGE_ORIENTATION_PATIENT)))
            .or(top_ori);
        let measures = functional(pf, shared, t::PIXEL_MEASURES_SEQUENCE);
        let spacing = measures
            .and_then(|i| spacing2(&i.numbers(t::PIXEL_SPACING)))
            .or(top_spacing);
        let thickness = measures
            .and_then(|i| i.number(t::SLICE_THICKNESS))
            .or_else(|| ds.number(t::SLICE_THICKNESS));
        let modality = functional(pf, shared, t::PIXEL_VALUE_TRANSFORMATION_SEQUENCE)
            .map_or_else(|| top_modality.clone(), |i| Modality::of(file, i));
        let content = pf.and_then(|p| p.first(t::FRAME_CONTENT_SEQUENCE));
        let temporal = content
            .and_then(|c| c.number(t::TEMPORAL_POSITION_INDEX))
            .or_else(|| ds.number(t::TEMPORAL_POSITION_IDENTIFIER));
        let window = functional(pf, shared, t::FRAME_VOI_LUT_SEQUENCE)
            .and_then(window_of)
            .or(top_window);
        let mut position = pos.or(top_pos);
        if pos.is_none()
            && let (Some(p), Some(o), Some(off)) = (top_pos, top_ori, offsets.get(f as usize))
            && offsets.len() == desc.frames as usize
        {
            let n = normal(&o);
            position = Some([p[0] + n[0] * off, p[1] + n[1] * off, p[2] + n[2] * off]);
        }
        // a classic multi-frame file has one position for all its frames: not per frame
        if pos.is_none() && desc.frames > 1 && offsets.len() != desc.frames as usize {
            position = None;
        }
        out.push(FrameRec {
            file: fi,
            frame: f,
            position,
            orientation: ori,
            spacing,
            thickness,
            modality,
            instance,
            temporal,
            trigger_ms: ds.number(t::TRIGGER_TIME),
            time_s: time_s.map(|s| s + frame_time.map_or(0.0, |ft| ft / 1000.0 * f64::from(f))),
            slice_location: ds.number(t::SLICE_LOCATION),
            window,
            segment: pf
                .and_then(|p| p.first(0x0062_000A))
                .and_then(|s| s.uint(0x0062_000B)),
        });
    }
    out
}

/// The first Window Center / Width pair of an item.
fn window_of(i: &Item) -> Option<(f64, f64)> {
    let c = i.number(t::WINDOW_CENTER)?;
    let w = i.number(t::WINDOW_WIDTH).filter(|w| *w > 0.0)?;
    Some((c, w))
}

/// How the frames of an image are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrangement {
    /// One frame.
    Single,
    /// Slices ordered along the normal: Z.
    Volume,
    /// Slices ordered along the normal, repeated over time: Z and T.
    VolumeSeries,
    /// Frames over time (cine): T.
    Cine,
    /// Frames without positions, in instance order: Z.
    Stack,
    /// Segmentation: segments as C, slice positions as Z.
    Segmentation,
}

impl Arrangement {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Arrangement::Single => "single frame",
            Arrangement::Volume => "volume",
            Arrangement::VolumeSeries => "volume series",
            Arrangement::Cine => "cine",
            Arrangement::Stack => "stack",
            Arrangement::Segmentation => "segmentation",
        }
    }
}

/// One image: frames arranged in Z and T, with geometry.
#[derive(Debug, Clone)]
pub(crate) struct ImagePlan {
    /// Frames in plane order (`(c * size_t + t) * size_z + z`); `None` where the file has
    /// no frame (a segment absent from a slice), read as zeros.
    pub(crate) planes: Vec<Option<FrameRec>>,
    pub(crate) size_c: u32,
    /// Channel names (segment labels).
    pub(crate) channel_names: Vec<String>,
    pub(crate) size_z: u32,
    pub(crate) size_t: u32,
    pub(crate) arrangement: Arrangement,
    pub(crate) desc: PixelDesc,
    pub(crate) out_type: PixelType,
    /// The file whose header describes the image.
    pub(crate) first_file: usize,
    /// Z step in mm (between neighbouring slice positions).
    pub(crate) z_step_mm: Option<f64>,
    /// Interval between time points in seconds.
    pub(crate) time_step_s: Option<f64>,
    /// Where the Z step came from.
    pub(crate) z_step_source: Option<&'static str>,
    /// Unit normal of the slices (row × column), when oriented.
    pub(crate) normal: Option<[f64; 3]>,
    pub(crate) palette: Option<Palette>,
    pub(crate) notes: Vec<String>,
}

/// Grouping key: frames that share it may form one image.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    series: String,
    rows: u32,
    cols: u32,
    spp: u32,
    bits: u32,
    photometric: String,
    orientation: Option<[i64; 6]>,
    /// Files that cannot share an image with others (multi-frame files, projection images).
    own: Option<usize>,
}

fn round_orientation(o: &[f64; 6]) -> [i64; 6] {
    o.map(|v| (v * 1000.0).round() as i64)
}

/// Group the frames of `files` (with their pixel descriptions) into images.
pub(crate) fn plan(files: &[(DicomFile, PixelDesc)]) -> Vec<ImagePlan> {
    let mut groups: BTreeMap<Key, Vec<FrameRec>> = BTreeMap::new();
    let mut order: Vec<Key> = Vec::new();
    for (fi, (f, desc)) in files.iter().enumerate() {
        let series = f.ds.text(t::SERIES_INSTANCE_UID).unwrap_or_default();
        let recs = frames_of(f, fi, desc);
        let positioned = recs
            .iter()
            .all(|r| r.position.is_some() && r.orientation.is_some());
        let own = (desc.frames > 1 || !positioned).then_some(fi);
        for r in recs {
            let key = Key {
                series: series.clone(),
                rows: desc.rows,
                cols: desc.cols,
                spp: desc.spp,
                bits: desc.bits_allocated,
                photometric: desc.photometric.clone(),
                orientation: r.orientation.as_ref().map(round_orientation),
                own,
            };
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(r);
        }
    }
    // images in the order their first file was found (series order within a folder)
    order
        .into_iter()
        .filter_map(|k| groups.remove(&k))
        .map(|frames| arrange(files, frames))
        .collect()
}

/// Order the frames of one group in Z and T.
#[allow(clippy::many_single_char_names)]
fn arrange(files: &[(DicomFile, PixelDesc)], mut frames: Vec<FrameRec>) -> ImagePlan {
    let first_file = frames.first().map_or(0, |f| f.file);
    let (file0, desc0) = &files[first_file];
    let mut desc = desc0.clone();
    let n = frames.len();
    desc.frames = n as u32;
    if frames.iter().any(|f| f.segment.is_some()) {
        return segmentation(file0, desc, frames, first_file);
    }
    let mut notes = Vec::new();
    let nrm = frames
        .first()
        .and_then(|f| f.orientation)
        .map(|o| normal(&o));
    let positioned = n > 1 && frames.iter().all(|f| f.position.is_some()) && nrm.is_some();
    let time_key = |f: &FrameRec| {
        (
            f.temporal.unwrap_or(0.0),
            f.trigger_ms.unwrap_or(0.0),
            f.time_s.unwrap_or(0.0),
            f.instance.unwrap_or(0.0),
            f.file as f64,
            f64::from(f.frame),
        )
    };
    let mut arrangement = if n == 1 {
        Arrangement::Single
    } else {
        Arrangement::Stack
    };
    let mut size_z = n as u32;
    let mut size_t = 1u32;
    let mut z_step = None;
    let mut z_source = None;
    if positioned {
        let nv = nrm.unwrap_or([0.0, 0.0, 1.0]);
        let d = |f: &FrameRec| f.position.map_or(0.0, |p| dot(p, nv));
        frames.sort_by(|a, b| {
            d(a).total_cmp(&d(b)).then_with(|| {
                time_key(a)
                    .partial_cmp(&time_key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        // distinct positions (within 1 µm)
        let mut levels: Vec<(f64, usize)> = Vec::new();
        for f in &frames {
            let v = d(f);
            match levels.last_mut() {
                Some((l, c)) if (v - *l).abs() < 1e-3 => *c += 1,
                _ => levels.push((v, 1)),
            }
        }
        let nz = levels.len();
        let per = n / nz;
        if nz == n {
            arrangement = Arrangement::Volume;
        } else if levels.iter().all(|(_, c)| *c == per) && per > 1 {
            arrangement = Arrangement::VolumeSeries;
            size_z = nz as u32;
            size_t = per as u32;
            // frames are sorted by position then time: reorder to t * nz + z
            let mut reordered = Vec::with_capacity(n);
            for tt in 0..per {
                for z in 0..nz {
                    reordered.push(frames[z * per + tt].clone());
                }
            }
            frames = reordered;
        } else {
            notes.push(format!(
                "{n} frames at {nz} slice positions that repeat unevenly: frames are listed as a stack in position order"
            ));
        }
        if nz > 1 {
            let steps: Vec<f64> = levels.windows(2).map(|w| w[1].0 - w[0].0).collect();
            if let Some(m) = median(&steps) {
                z_step = Some(m);
                z_source = Some("slice positions");
                let irregular = steps
                    .iter()
                    .any(|s| (s - m).abs() > 0.01 * m.abs().max(1e-3));
                if irregular {
                    notes.push(format!(
                        "slice spacing is irregular ({:.4} to {:.4} mm); physical_size.z is the median {m:.4} mm",
                        steps.iter().copied().fold(f64::INFINITY, f64::min),
                        steps.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                    ));
                }
            }
        }
    } else if n > 1 {
        frames.sort_by(|a, b| {
            (a.file, a.instance.unwrap_or(0.0), a.frame)
                .partial_cmp(&(b.file, b.instance.unwrap_or(0.0), b.frame))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let ds = &file0.ds;
        let pointer = ds
            .get(t::FRAME_INCREMENT_POINTER)
            .map(super::parse::Element::tags)
            .unwrap_or_default();
        let modality = ds.text(t::MODALITY).unwrap_or_default();
        let temporal = pointer
            .iter()
            .any(|p| matches!(*p, 0x0018_1063 | 0x0018_1065 | 0x0018_1060))
            || ds.number(t::FRAME_TIME).is_some()
            || matches!(modality.as_str(), "US" | "XA" | "RF" | "ES" | "XC" | "IVUS");
        let spatial = pointer
            .iter()
            .any(|p| matches!(*p, 0x0054_0080 | 0x3004_000C));
        if temporal && !spatial {
            arrangement = Arrangement::Cine;
            size_z = 1;
            size_t = n as u32;
        }
    }
    // Z step fallbacks: SpacingBetweenSlices, then SliceThickness
    if z_step.is_none() && size_z > 1 {
        if let Some(s) = file0
            .ds
            .number(t::SPACING_BETWEEN_SLICES)
            .filter(|v| *v > 0.0)
        {
            z_step = Some(s);
            z_source = Some("SpacingBetweenSlices");
        } else if let Some(s) = frames
            .first()
            .and_then(|f| f.thickness)
            .filter(|v| *v > 0.0)
        {
            z_step = Some(s);
            z_source = Some("SliceThickness");
        }
    }
    // time step
    let mut time_step = file0
        .ds
        .number(t::FRAME_TIME)
        .filter(|v| *v > 0.0)
        .map(|ms| ms / 1000.0);
    if time_step.is_none() && size_t > 1 {
        let tz = size_z as usize;
        let times: Vec<f64> = (0..size_t as usize)
            .filter_map(|tt| frames.get(tt * tz))
            .map(|f| {
                f.trigger_ms
                    .map(|ms| ms / 1000.0)
                    .or(f.time_s)
                    .unwrap_or(f64::NAN)
            })
            .collect();
        let steps: Vec<f64> = times
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|v| *v > 0.0)
            .collect();
        time_step = median(&steps);
    }
    let modalities: Vec<&Modality> = frames.iter().map(|f| &f.modality).collect();
    let out_type = output_type(&desc, &modalities);
    let palette = (desc.photometric == "PALETTE COLOR")
        .then(|| Palette::of(file0, &file0.ds))
        .and_then(|p| match p {
            Ok(p) => Some(p),
            Err(e) => {
                notes.push(format!("palette colour: {e}; indices are returned"));
                None
            }
        });
    ImagePlan {
        planes: frames.into_iter().map(Some).collect(),
        size_c: 1,
        channel_names: Vec::new(),
        size_z,
        size_t,
        arrangement,
        desc,
        out_type,
        first_file,
        z_step_mm: z_step.map(f64::abs),
        time_step_s: time_step,
        z_step_source: z_source,
        normal: nrm,
        palette,
        notes,
    }
}

/// A segmentation (PS3.3 A.51): each segment is a channel, and the union of the slice
/// positions its frames sit at is Z. A segment absent from a slice has no frame there.
fn segmentation(
    file0: &DicomFile,
    desc: PixelDesc,
    frames: Vec<FrameRec>,
    first_file: usize,
) -> ImagePlan {
    let nrm = frames
        .first()
        .and_then(|f| f.orientation)
        .map(|o| normal(&o));
    let nv = nrm.unwrap_or([0.0, 0.0, 1.0]);
    let d = |f: &FrameRec| f.position.map_or(f64::from(f.frame), |p| dot(p, nv));
    let mut levels: Vec<f64> = frames.iter().map(d).collect();
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    let mut segs: Vec<u32> = frames.iter().map(|f| f.segment.unwrap_or(0)).collect();
    segs.sort_unstable();
    segs.dedup();
    let (nz, nc) = (levels.len().max(1), segs.len().max(1));
    let mut planes: Vec<Option<FrameRec>> = vec![None; nz * nc];
    for f in frames {
        let c = segs
            .iter()
            .position(|s| *s == f.segment.unwrap_or(0))
            .unwrap_or(0);
        let v = d(&f);
        let z = levels
            .iter()
            .position(|l| (l - v).abs() < 1e-3)
            .unwrap_or(0);
        let slot = &mut planes[c * nz + z];
        if slot.is_none() {
            *slot = Some(f);
        }
    }
    let labels: Vec<(u32, String)> = file0
        .ds
        .items(0x0062_0002)
        .iter()
        .filter_map(|i| {
            Some((
                i.uint(0x0062_0004)?,
                i.text(0x0062_0005).unwrap_or_default(),
            ))
        })
        .collect();
    let channel_names = segs
        .iter()
        .map(|s| {
            labels
                .iter()
                .find(|(n, _)| n == s)
                .map_or_else(|| format!("segment {s}"), |(_, l)| l.clone())
        })
        .collect();
    let steps: Vec<f64> = levels.windows(2).map(|w| w[1] - w[0]).collect();
    let z_step = median(&steps).or_else(|| file0.ds.number(t::SPACING_BETWEEN_SLICES));
    let modalities = [&Modality::Identity];
    ImagePlan {
        planes,
        size_c: nc as u32,
        channel_names,
        size_z: nz as u32,
        size_t: 1,
        arrangement: Arrangement::Segmentation,
        out_type: output_type(&desc, &modalities),
        desc,
        first_file,
        z_step_mm: z_step.map(f64::abs).filter(|v| *v > 0.0),
        time_step_s: None,
        z_step_source: Some("slice positions"),
        normal: nrm,
        palette: None,
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tm_values() {
        assert_eq!(tm_seconds("101530.5"), Some(36930.5));
        assert_eq!(tm_seconds("10:15:30"), Some(36930.0));
        assert_eq!(tm_seconds("1015"), Some(36900.0));
        assert_eq!(tm_seconds("x"), None);
    }

    #[test]
    fn axial_normal_points_head_wards() {
        let n = normal(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(n, [0.0, 0.0, 1.0]);
    }
}
