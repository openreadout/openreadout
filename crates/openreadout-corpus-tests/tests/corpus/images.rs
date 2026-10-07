//! Images: geometry, pixel types, physical sizes and per-plane hashes; ND2 metadata and mosaic
//! placement.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::reader::PlaneIndex;

use crate::oracle::{Nd2Meta, Oracle, OracleMosaic, OraclePlane};
use crate::{LOSSY_MEAN_TOLERANCE, covered};

/// Mean of a plane's samples (all samples, interleaved channels included).
pub(crate) fn plane_mean(p: &openreadout_core::Plane) -> f64 {
    use openreadout_core::PixelType as P;
    let d = &p.data;
    let vals: Vec<f64> = match p.pixel_type {
        P::Uint8 => d.iter().map(|&v| f64::from(v)).collect(),
        P::Int8 => d.iter().map(|&v| f64::from(v as i8)).collect(),
        P::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes(*c)))
            .collect(),
        P::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes(*c)))
            .collect(),
        P::Uint32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(u32::from_le_bytes(*c)))
            .collect(),
        P::Int32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(i32::from_le_bytes(*c)))
            .collect(),
        P::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes(*c)))
            .collect(),
        P::Double => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect(),
        other => panic!("plane_mean: add pixel type {other:?}"),
    };
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64
    }
}

pub(crate) fn approx(a: Option<f64>, b: Option<&Option<f64>>) -> bool {
    match (a, b.copied().flatten()) {
        (Some(a), Some(b)) => {
            (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-9) || (a - b).abs() < 1e-9
        }
        (None, None) => true,
        _ => false,
    }
}

/// Largest per-sample difference between a plane and the oracle's raw sidecar, if it exists.
pub(crate) fn sidecar_diff(
    path: &Path,
    image: u32,
    p: &OraclePlane,
    plane: &openreadout_core::Plane,
) -> Option<u64> {
    sidecar_diff_level(path, image, 0, p, plane)
}

/// Like `sidecar_diff` for pyramid level `level` (`image<i>_l<level>_c…` when level > 0).
pub(crate) fn sidecar_diff_level(
    path: &Path,
    image: u32,
    level: u32,
    p: &OraclePlane,
    plane: &openreadout_core::Plane,
) -> Option<u64> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let prefix = if level == 0 {
        format!("image{image}")
    } else {
        format!("image{image}_l{level}")
    };
    let f = path
        .with_file_name(format!("{stem}.oracle"))
        .join(format!("{prefix}_c{}_z{}_t{}.bin", p.c, p.z, p.t));
    let want = std::fs::read(f).ok()?;
    if want.len() != plane.data.len() {
        return Some(u64::MAX);
    }
    let bps = plane.pixel_type.bytes_per_sample();
    let mut max = 0u64;
    for (a, b) in plane.data.chunks(bps).zip(want.chunks(bps)) {
        let v = |x: &[u8]| {
            x.iter()
                .rev()
                .fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
        };
        max = max.max(v(a).abs_diff(v(b)));
    }
    Some(max)
}

/// What the image comparison found: planes equal to the oracle's (by hash, within the
/// sidecar tolerance, by mean for lossy files, at pyramid levels), mosaic tiles placed, FLIM
/// images refused as expected and names that match.
#[derive(Default)]
pub(crate) struct ImageTally {
    pub(crate) ok_planes: usize,
    pub(crate) tol_planes: usize,
    pub(crate) lossy_planes: usize,
    /// Largest per-sample difference of a plane within tolerance.
    pub(crate) worst: u64,
    pub(crate) level_planes: usize,
    pub(crate) ok_tiles: usize,
    pub(crate) flim_ok: usize,
    pub(crate) names_ok: usize,
}

/// Compare the images: count, geometry, pixel type, physical sizes, names, every oracle plane
/// (hash, else the sidecar tolerance, else the mean for lossy files) and pyramid level, mosaic
/// placement and FLIM refusal. Differences go to `problems`.
pub(crate) fn compare_images(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
    oracle: &Oracle,
    path: &Path,
    tolerance: Option<u32>,
    lossy: bool,
    problems: &mut Vec<String>,
) -> ImageTally {
    let mut ok_planes = 0usize;
    let mut tol_planes = 0usize;
    let mut lossy_planes = 0usize;
    let mut worst = 0u64;
    let mut level_planes = 0usize;
    let mut ok_tiles = 0usize;
    let mut flim_ok = 0usize;
    let mut names_ok = 0usize;
    if info.images.len() != oracle.images.len() {
        problems.push(format!(
            "image count {} != oracle {}",
            info.images.len(),
            oracle.images.len()
        ));
    }
    for o in &oracle.images {
        let Some(im) = info.images.iter().find(|i| i.index == o.index) else {
            problems.push(format!("image {} missing", o.index));
            continue;
        };
        if o.skip.is_some() {
            continue;
        }
        let geo = (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t);
        let ogeo = (o.size_x, o.size_y, o.size_z, o.size_c, o.size_t);
        if geo != ogeo {
            problems.push(format!(
                "image {}: geometry {geo:?} != oracle {ogeo:?}",
                o.index
            ));
        }
        if im.pixel_type.ome_name() != o.pixel_type {
            problems.push(format!(
                "image {}: pixel type {} != oracle {}",
                o.index,
                im.pixel_type.ome_name(),
                o.pixel_type
            ));
        }
        for (axis, ours) in [
            ("x", im.physical_size.x),
            ("y", im.physical_size.y),
            ("z", im.physical_size.z),
        ] {
            let theirs = o.physical_size_um.get(axis);
            if axis == "z" && o.size_z <= 1 {
                continue; // a Z step is meaningless for a single plane; some oracles report a default of 1.0
            }
            // An oracle's 0.0 is "not calibrated", which we omit (book/src/guides/metadata.md).
            let theirs = theirs.filter(|t| t.is_none_or(|v| v > 0.0));
            if theirs.is_some() && !approx(ours, theirs) {
                problems.push(format!(
                    "image {}: physical_size.{axis} {ours:?} != oracle {theirs:?}",
                    o.index
                ));
            }
        }
        if let Some(n) = &o.check_name {
            if im.name.as_deref() == Some(n.as_str()) {
                names_ok += 1;
            } else {
                problems.push(format!(
                    "image {}: name {:?} != oracle {n:?}",
                    o.index, im.name
                ));
            }
        }
        if let Some(names) = &o.check_channel_names {
            let ours: Vec<String> = im
                .channels
                .iter()
                .map(|c| c.name.clone().unwrap_or_default())
                .collect();
            if &ours == names {
                names_ok += ours.len();
            } else {
                problems.push(format!(
                    "image {}: channel names {ours:?} != oracle {names:?}",
                    o.index
                ));
            }
        }
        if !o.modulo.is_empty() {
            let ours: BTreeMap<String, u32> = im
                .extra
                .get("modulo")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    Some((
                        m.get("along")?.as_str()?.to_string(),
                        u32::try_from(m.get("size")?.as_u64()?).ok()?,
                    ))
                })
                .collect();
            if ours == o.modulo {
                names_ok += ours.len();
            } else {
                problems.push(format!(
                    "image {}: modulo sizes {ours:?} != oracle {:?}",
                    o.index, o.modulo
                ));
            }
        }
        if geo != ogeo {
            continue;
        }
        if o.flim {
            match ds.read_plane(o.index, PlaneIndex::default()) {
                Err(openreadout_core::Error::Unsupported { .. }) => flim_ok += 1,
                other => problems.push(format!(
                    "image {}: FLIM plane read should be unsupported, got {:?}",
                    o.index,
                    other.map(|p| p.data.len())
                )),
            }
            continue;
        }
        if let Some(m) = &o.mosaic {
            check_mosaic(ds, im, o.index, m, problems, &mut ok_tiles);
        }
        for p in &o.planes {
            match ds.read_plane(
                o.index,
                PlaneIndex {
                    c: p.c,
                    z: p.z,
                    t: p.t,
                },
            ) {
                Ok(plane) => {
                    let h = plane.xxh3_hex();
                    let within = tolerance.and_then(|tol| {
                        sidecar_diff(path, o.index, p, &plane).filter(|d| *d <= u64::from(tol))
                    });
                    if h == p.xxh3 {
                        ok_planes += 1;
                    } else if let Some(d) = within {
                        tol_planes += 1;
                        worst = worst.max(d);
                    } else if lossy
                        && p.mean
                            .is_some_and(|m| (plane_mean(&plane) - m).abs() <= LOSSY_MEAN_TOLERANCE)
                    {
                        lossy_planes += 1;
                    } else {
                        problems.push(format!(
                            "image {} c={} z={} t={}: hash {h} != oracle {}",
                            o.index, p.c, p.z, p.t, p.xxh3
                        ));
                        if problems.len() > 12 {
                            problems.push("(more omitted)".into());
                            break;
                        }
                    }
                }
                Err(e) => {
                    problems.push(format!(
                        "image {} c={} z={} t={}: read failed: {e}",
                        o.index, p.c, p.z, p.t
                    ));
                    break;
                }
            }
        }
    }
    for o in &oracle.images {
        for l in &o.levels {
            for p in &l.planes {
                match ds.read_plane_level(
                    o.index,
                    PlaneIndex {
                        c: p.c,
                        z: p.z,
                        t: p.t,
                    },
                    l.level,
                ) {
                    Ok(plane) if (plane.width, plane.height) != (l.size_x, l.size_y) => {
                        problems.push(format!(
                            "image {} level {}: {}x{} != oracle {}x{}",
                            o.index, l.level, plane.width, plane.height, l.size_x, l.size_y
                        ));
                        break;
                    }
                    Ok(plane) if plane.xxh3_hex() == p.xxh3 => level_planes += 1,
                    Ok(plane)
                        if tolerance.is_some_and(|tol| {
                            sidecar_diff_level(path, o.index, l.level, p, &plane)
                                .is_some_and(|d| d <= u64::from(tol))
                        }) =>
                    {
                        let d = sidecar_diff_level(path, o.index, l.level, p, &plane).unwrap_or(0);
                        tol_planes += 1;
                        worst = worst.max(d);
                    }
                    // Lossy files (JPEG pyramids): the plane mean, as for full-resolution planes.
                    Ok(plane)
                        if lossy
                            && p.mean.is_some_and(|m| {
                                (plane_mean(&plane) - m).abs() <= LOSSY_MEAN_TOLERANCE
                            }) =>
                    {
                        lossy_planes += 1;
                    }
                    Ok(plane) => {
                        problems.push(format!(
                            "image {} level {} c={} z={} t={}: hash {} != oracle {}",
                            o.index,
                            l.level,
                            p.c,
                            p.z,
                            p.t,
                            plane.xxh3_hex(),
                            p.xxh3
                        ));
                        break;
                    }
                    Err(e) => {
                        problems.push(format!(
                            "image {} level {}: read failed: {e}",
                            o.index, l.level
                        ));
                        break;
                    }
                }
            }
        }
    }
    ImageTally {
        ok_planes,
        tol_planes,
        lossy_planes,
        worst,
        level_planes,
        ok_tiles,
        flim_ok,
        names_ok,
    }
}

pub(crate) fn close(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0),
        (None, None) => true,
        _ => false,
    }
}

/// The record field our ND2 reader uses for a custom-data tag (see docs/formats/nd2.md).
pub(crate) fn nd2_field(tag: &str, z_tag: Option<&str>) -> Option<&'static str> {
    if Some(tag) == z_tag {
        return Some("stage_z_um");
    }
    Some(match tag {
        "X" => "stage_x_um",
        "Y" => "stage_y_um",
        "PFS_STATUS" => "pfs_status",
        "PFS_OFFSET" => "pfs_offset",
        "Camera_ExposureTime1" => "exposure_ms",
        "CameraTemp1" => "camera_temperature_c",
        _ => return None,
    })
}

/// ND2 metadata against the `nd2` package: channel names, colours and wavelengths, the
/// acquisition start, and every sampled per-frame record (time, stage position, custom columns).
/// Legacy files: the oracle reports no wavelengths, so only names and frames count.
pub(crate) fn check_nd2_meta(
    ds: &dyn openreadout_core::reader::Dataset,
    info: &openreadout_core::FileInfo,
    m: &Nd2Meta,
    legacy: bool,
) -> (usize, Vec<String>) {
    let mut ok = 0usize;
    let mut problems = Vec::new();
    if let Some(img) = info.images.first() {
        for (ours, theirs) in img.channels.iter().zip(&m.channels) {
            let name = ours.name.clone().unwrap_or_default();
            let their_name = theirs.name.clone().unwrap_or_default();
            if name == their_name || (legacy && their_name.is_empty()) {
                ok += 1;
            } else {
                problems.push(format!(
                    "channel {} name {name:?} != {their_name:?}",
                    ours.index
                ));
            }
            if ours.color.as_deref() == Some(theirs.color.as_str()) {
                ok += 1;
            } else {
                problems.push(format!(
                    "channel {} color {:?} != {}",
                    ours.index, ours.color, theirs.color
                ));
            }
            if theirs.component_count == 3 {
                continue; // nd2 assigns pseudo-wavelengths to RGB planes; we report none
            }
            for (what, a, b) in [
                ("excitation", ours.excitation_nm, theirs.excitation_nm),
                ("emission", ours.emission_nm, theirs.emission_nm),
            ] {
                // nd2 ignores the integral `uiWavelength` of older files and all legacy
                // wavelengths: None there is not a disagreement.
                if close(a, b) || (b.is_none() && a.is_some()) {
                    ok += 1;
                } else {
                    problems.push(format!("channel {} {what} {a:?} != {b:?}", ours.index));
                }
            }
        }
        if let Some(a) = &m.acquired_at {
            covered("experiment.acquisition.started_at");
            if img.acquired_at.as_deref() == Some(a.as_str()) {
                ok += 1;
            } else {
                problems.push(format!("acquired_at {:?} != {a}", img.acquired_at));
            }
        }
    }
    let mut records: BTreeMap<u64, serde_json::Value> = BTreeMap::new();
    for img in &info.images {
        match ds.frames(img.index, None) {
            Ok((_, recs)) => {
                for r in recs {
                    if let Some(f) = r.get("frame").and_then(serde_json::Value::as_u64) {
                        records.insert(f, r);
                    }
                }
            }
            Err(e) => problems.push(format!("frames of image {}: {e}", img.index)),
        }
    }
    let addressed = records.len() as u64;
    for f in &m.frames {
        let Some(r) = records.get(&u64::from(f.index)) else {
            if legacy && u64::from(f.index) >= addressed {
                continue; // a frame past the loop tree (b16-14-12 holds one more than it describes)
            }
            problems.push(format!("frame {} has no record", f.index));
            continue;
        };
        let num = |k: &str| r.get(k).and_then(serde_json::Value::as_f64);
        let mut bad = Vec::new();
        if !close(num("time_ms"), f.time_ms) {
            bad.push(format!("time_ms {:?} != {:?}", num("time_ms"), f.time_ms));
        }
        for (k, v) in [
            ("stage_x_um", f.stage_x_um),
            ("stage_y_um", f.stage_y_um),
            ("stage_z_um", f.stage_z_um),
        ] {
            if v.is_some() && !close(num(k), v) {
                bad.push(format!("{k} {:?} != {v:?}", num(k)));
            }
        }
        let z_tag = if f.tags.contains_key("Z") {
            Some("Z".to_string())
        } else {
            f.tags
                .keys()
                .filter(|t| {
                    t.len() > 1 && t.starts_with('Z') && t[1..].chars().all(|c| c.is_ascii_digit())
                })
                .min_by_key(|t| t[1..].parse::<u32>().unwrap_or(u32::MAX))
                .cloned()
        };
        for (tag, v) in &f.tags {
            let got = match nd2_field(tag, z_tag.as_deref()) {
                Some(field) => r.get(field),
                None => r.get("tags").and_then(|t| t.get(tag)),
            };
            let same = match (got.and_then(serde_json::Value::as_f64), v.as_f64()) {
                (Some(a), Some(b)) => close(Some(a), Some(b)),
                _ => got == Some(v),
            };
            if !same {
                bad.push(format!("{tag} {got:?} != {v}"));
            }
        }
        if bad.is_empty() {
            ok += 1;
        } else {
            problems.push(format!("frame {}: {}", f.index, bad.join(", ")));
        }
    }
    (ok, problems)
}

/// Mosaic placement check, independent of the stitched-plane hash: our reported tile offsets must
/// equal the oracle's, and the part of every tile that no later tile covers must appear unchanged
/// in our stitched (c0, z0, t0) plane at its offset.
pub(crate) fn check_mosaic(
    ds: &mut dyn openreadout_core::reader::Dataset,
    im: &openreadout_core::model::ImageInfo,
    index: u32,
    m: &OracleMosaic,
    problems: &mut Vec<String>,
    ok_tiles: &mut usize,
) {
    let ours: Vec<(u64, u64)> = im
        .extra
        .get("tiles")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|t| {
                    (
                        t["x_px"].as_u64().unwrap_or(u64::MAX),
                        t["y_px"].as_u64().unwrap_or(u64::MAX),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let theirs: Vec<(u64, u64)> = m
        .tiles
        .iter()
        .map(|t| (u64::from(t.x_px), u64::from(t.y_px)))
        .collect();
    if ours != theirs {
        problems.push(format!("image {index}: tile offsets differ from oracle"));
        return;
    }
    let plane = match ds.read_plane(index, PlaneIndex::default()) {
        Ok(p) => p,
        Err(e) => {
            problems.push(format!("image {index}: stitched plane read failed: {e}"));
            return;
        }
    };
    let bps = plane.pixel_type.bytes_per_sample();
    let row = plane.width as usize * bps;
    for t in &m.tiles {
        let (rect, want) = match (&t.visible, &t.xxh3_visible, &t.xxh3_c0z0t0) {
            (Some(r), Some(h), _) => (*r, h),
            (None, _, Some(h)) if t.fully_visible => ([0, 0, m.tile_width, m.tile_height], h),
            _ => continue,
        };
        let [vx, vy, vw, vh] = rect.map(|v| v as usize);
        let mut crop = Vec::with_capacity(vw * vh * bps);
        for y in 0..vh {
            let start = (t.y_px as usize + vy + y) * row + (t.x_px as usize + vx) * bps;
            crop.extend_from_slice(&plane.data[start..start + vw * bps]);
        }
        let got = openreadout_core::Plane {
            width: rect[2],
            height: rect[3],
            pixel_type: plane.pixel_type,
            samples_per_pixel: 1,
            data: crop,
        }
        .xxh3_hex();
        if &got == want {
            *ok_tiles += 1;
        } else {
            problems.push(format!(
                "image {index} tile {}: region at ({}, {}) hash {got} != tile hash {want}",
                t.index, t.x_px, t.y_px
            ));
        }
    }
}
