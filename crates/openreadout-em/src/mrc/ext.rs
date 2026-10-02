//! Extended headers: FEI1/FEI2 metadata blocks (layout from mrcfile's BSD-licensed `dtypes.py`),
//! SerialEM `SERI` section records and Agard-style integer/real records (IMOD documentation), and
//! CCP4 symmetry text. Field names are ours; see `docs/formats/mrc.md`.

use serde_json::{Map, Value, json};

use openreadout_core::bytes::{Endian, le_u32};

use crate::util::{num, text};

/// How one FEI metadata field is stored.
#[derive(Debug, Clone, Copy)]
enum Kind {
    F64,
    I32,
    I64,
    /// Always little-endian, whatever the header's byte order.
    Bitmask,
    Bool,
    Text(usize),
}

/// (our name, byte offset, storage). FEI1 blocks end at 768; FEI2 adds the fields from 768 on.
const FEI_FIELDS: &[(&str, usize, Kind)] = &[
    ("metadata_size", 0, Kind::I32),
    ("metadata_version", 4, Kind::I32),
    ("bitmask_1", 8, Kind::Bitmask),
    ("timestamp", 12, Kind::F64),
    ("microscope_type", 20, Kind::Text(16)),
    ("d_number", 36, Kind::Text(16)),
    ("application", 52, Kind::Text(16)),
    ("application_version", 68, Kind::Text(16)),
    ("high_tension", 84, Kind::F64),
    ("dose", 92, Kind::F64),
    ("alpha_tilt", 100, Kind::F64),
    ("beta_tilt", 108, Kind::F64),
    ("stage_x", 116, Kind::F64),
    ("stage_y", 124, Kind::F64),
    ("stage_z", 132, Kind::F64),
    ("tilt_axis_angle", 140, Kind::F64),
    ("dual_axis_rotation", 148, Kind::F64),
    ("pixel_size_x", 156, Kind::F64),
    ("pixel_size_y", 164, Kind::F64),
    ("defocus", 220, Kind::F64),
    ("stem_defocus", 228, Kind::F64),
    ("applied_defocus", 236, Kind::F64),
    ("instrument_mode", 244, Kind::I32),
    ("projection_mode", 248, Kind::I32),
    ("objective_lens_mode", 252, Kind::Text(16)),
    ("high_magnification_mode", 268, Kind::Text(16)),
    ("probe_mode", 284, Kind::I32),
    ("eftem_on", 288, Kind::Bool),
    ("magnification", 289, Kind::F64),
    ("bitmask_2", 297, Kind::Bitmask),
    ("camera_length", 301, Kind::F64),
    ("spot_index", 309, Kind::I32),
    ("illuminated_area", 313, Kind::F64),
    ("intensity", 321, Kind::F64),
    ("convergence_angle", 329, Kind::F64),
    ("illumination_mode", 337, Kind::Text(16)),
    ("wide_convergence_angle_range", 353, Kind::Bool),
    ("slit_inserted", 354, Kind::Bool),
    ("slit_width", 355, Kind::F64),
    ("acceleration_voltage_offset", 363, Kind::F64),
    ("drift_tube_voltage", 371, Kind::F64),
    ("energy_shift", 379, Kind::F64),
    ("shift_offset_x", 387, Kind::F64),
    ("shift_offset_y", 395, Kind::F64),
    ("shift_x", 403, Kind::F64),
    ("shift_y", 411, Kind::F64),
    ("integration_time", 419, Kind::F64),
    ("binning_width", 427, Kind::I32),
    ("binning_height", 431, Kind::I32),
    ("camera_name", 435, Kind::Text(16)),
    ("readout_area_left", 451, Kind::I32),
    ("readout_area_top", 455, Kind::I32),
    ("readout_area_right", 459, Kind::I32),
    ("readout_area_bottom", 463, Kind::I32),
    ("ceta_noise_reduction", 467, Kind::Bool),
    ("ceta_frames_summed", 468, Kind::I32),
    ("direct_detector_electron_counting", 472, Kind::Bool),
    ("direct_detector_align_frames", 473, Kind::Bool),
    ("bitmask_3", 490, Kind::Bitmask),
    ("phase_plate", 518, Kind::Bool),
    ("stem_detector_name", 519, Kind::Text(16)),
    ("stem_gain", 535, Kind::F64),
    ("stem_offset", 543, Kind::F64),
    ("dwell_time", 571, Kind::F64),
    ("frame_time", 579, Kind::F64),
    ("scan_size_left", 587, Kind::I32),
    ("scan_size_top", 591, Kind::I32),
    ("scan_size_right", 595, Kind::I32),
    ("scan_size_bottom", 599, Kind::I32),
    ("full_scan_fov_x", 603, Kind::F64),
    ("full_scan_fov_y", 611, Kind::F64),
    ("element", 619, Kind::Text(16)),
    ("energy_interval_lower", 635, Kind::F64),
    ("energy_interval_higher", 643, Kind::F64),
    ("method", 651, Kind::I32),
    ("is_dose_fraction", 655, Kind::Bool),
    ("fraction_number", 656, Kind::I32),
    ("start_frame", 660, Kind::I32),
    ("end_frame", 664, Kind::I32),
    ("input_stack_filename", 668, Kind::Text(80)),
    ("bitmask_4", 748, Kind::Bitmask),
    ("alpha_tilt_min", 752, Kind::F64),
    ("alpha_tilt_max", 760, Kind::F64),
    // FEI2 (metadata version 2) additions
    ("scan_rotation", 768, Kind::F64),
    ("diffraction_pattern_rotation", 776, Kind::F64),
    ("image_rotation", 784, Kind::F64),
    ("scan_mode", 792, Kind::I32),
    ("acquisition_time_stamp", 796, Kind::I64),
    ("detector_commercial_name", 804, Kind::Text(16)),
    ("start_tilt_angle", 820, Kind::F64),
    ("end_tilt_angle", 828, Kind::F64),
    ("tilt_per_image", 836, Kind::F64),
    ("tilt_speed", 844, Kind::F64),
    ("beam_center_x_pixel", 852, Kind::I32),
    ("beam_center_y_pixel", 856, Kind::I32),
    ("cfeg_flash_timestamp", 860, Kind::I64),
    ("phase_plate_position_index", 868, Kind::I32),
    ("objective_aperture_name", 872, Kind::Text(16)),
];

/// Bytes of an FEI1 block.
pub const FEI1_BLOCK_LEN: usize = 768;
/// Bytes of the FEI2 (metadata version 2) block.
pub const FEI2_BLOCK_LEN: usize = 888;

fn field_len(k: Kind) -> usize {
    match k {
        Kind::F64 | Kind::I64 => 8,
        Kind::I32 | Kind::Bitmask => 4,
        Kind::Bool => 1,
        Kind::Text(n) => n,
    }
}

/// Decode one FEI metadata block (every field that fits in `block`, set or not).
pub(crate) fn fei_block(block: &[u8], e: Endian) -> Map<String, Value> {
    let mut m = Map::new();
    for &(name, off, kind) in FEI_FIELDS {
        if off + field_len(kind) > block.len() {
            continue;
        }
        let v = match kind {
            Kind::F64 => e.f64(block, off).map_or(Value::Null, num),
            Kind::I32 => e.i32(block, off).map_or(Value::Null, Value::from),
            Kind::I64 => e.i64(block, off).map_or(Value::Null, Value::from),
            Kind::Bitmask => le_u32(block, off).map_or(Value::Null, Value::from),
            Kind::Bool => Value::Bool(block[off] != 0),
            Kind::Text(n) => Value::String(text(&block[off..off + n])),
        };
        m.insert(name.to_string(), v);
    }
    m
}

/// The FEI block size recorded in the first block, if it is sane for this extended header.
pub(crate) fn fei_block_len(ext: &[u8], e: Endian) -> Option<usize> {
    let n = usize::try_from(e.i32(ext, 0)?).ok()?;
    (n >= FEI1_BLOCK_LEN && n <= ext.len() && n <= 1 << 20).then_some(n)
}

/// SerialEM's two-short float (IMOD documentation).
fn seri_float(s1: i16, s2: i16) -> f64 {
    let a1 = f64::from(s1.unsigned_abs());
    let a2 = i32::from(s2.unsigned_abs());
    let mant = a1 * 256.0 + f64::from(a2 % 256);
    let exp = f64::from(s2.signum()) * f64::from(a2 / 256);
    f64::from(s1.signum()) * mant * 2f64.powf(exp)
}

/// (flag, bytes) of SerialEM section items, in storage order.
const SERI_ITEMS: [(i16, usize); 11] = [
    (1, 2),
    (2, 6),
    (4, 4),
    (8, 2),
    (16, 2),
    (32, 4),
    (64, 2),
    (128, 4),
    (256, 2),
    (512, 4),
    (1024, 2),
];

/// Bytes per section implied by SerialEM flags, if `nreal` is a SerialEM flag word.
pub(crate) fn seri_bytes(nreal: i16) -> Option<usize> {
    if !(0..2048).contains(&nreal) {
        return None;
    }
    Some(
        SERI_ITEMS
            .iter()
            .filter(|(f, _)| nreal & f != 0)
            .map(|(_, n)| n)
            .sum(),
    )
}

/// Decode one SerialEM section record.
pub(crate) fn seri_record(rec: &[u8], nreal: i16, e: Endian) -> Map<String, Value> {
    let mut m = Map::new();
    let mut off = 0usize;
    let s = |o: usize| e.i16(rec, o).unwrap_or(0);
    for (flag, n) in SERI_ITEMS {
        if nreal & flag == 0 {
            continue;
        }
        if off + n > rec.len() {
            break;
        }
        match flag {
            1 => {
                m.insert("tilt_angle_deg".into(), num(f64::from(s(off)) / 100.0));
            }
            2 => {
                let u = |o: usize| e.u16(rec, o).unwrap_or(0);
                m.insert(
                    "piece_coordinates".into(),
                    json!([u(off), u(off + 2), u(off + 4)]),
                );
            }
            4 => {
                m.insert("stage_x_um".into(), num(f64::from(s(off)) / 25.0));
                m.insert("stage_y_um".into(), num(f64::from(s(off + 2)) / 25.0));
            }
            8 => {
                m.insert("magnification".into(), num(f64::from(s(off)) * 100.0));
            }
            16 => {
                m.insert("intensity".into(), num(f64::from(s(off)) / 25_000.0));
            }
            32 => {
                m.insert(
                    "exposure_dose_e_per_a2".into(),
                    num(seri_float(s(off), s(off + 2))),
                );
            }
            _ => {}
        }
        off += n;
    }
    m
}

/// Agard/FEI-style record: `nint` integers then `nreal` floats.
pub(crate) fn ints_reals_record(
    rec: &[u8],
    nint: usize,
    nreal: usize,
    e: Endian,
) -> Map<String, Value> {
    let ints: Vec<Value> = (0..nint)
        .filter_map(|k| e.i32(rec, 4 * k).map(Value::from))
        .collect();
    let reals: Vec<Value> = (0..nreal)
        .filter_map(|k| e.f32(rec, 4 * (nint + k)).map(|v| num(f64::from(v))))
        .collect();
    let mut m = Map::new();
    m.insert("integers".into(), Value::Array(ints));
    m.insert("reals".into(), Value::Array(reals));
    m
}

/// CCP4 symmetry records: 80-character text lines.
pub(crate) fn symmetry_lines(ext: &[u8]) -> Vec<String> {
    ext.chunks(80).map(text).filter(|l| !l.is_empty()).collect()
}

/// Text-like extended header: mostly printable ASCII.
pub(crate) fn is_text(ext: &[u8]) -> bool {
    !ext.is_empty()
        && ext
            .iter()
            .filter(|&&c| c == 0 || c == b' ' || c.is_ascii_graphic())
            .count()
            == ext.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fei_fields_do_not_overlap() {
        let mut end = 0;
        for &(name, off, kind) in FEI_FIELDS {
            assert!(off >= end, "{name} overlaps the previous field");
            end = off + field_len(kind);
        }
        assert_eq!(end, FEI2_BLOCK_LEN);
    }

    #[test]
    fn fei_block_decodes_little_endian_with_le_bitmasks() {
        let mut b = vec![0u8; FEI1_BLOCK_LEN];
        b[0..4].copy_from_slice(&768i32.to_be_bytes());
        b[8..12].copy_from_slice(&5u32.to_le_bytes());
        b[84..92].copy_from_slice(&300_000f64.to_be_bytes());
        b[435..441].copy_from_slice(b"Falcon");
        let m = fei_block(&b, Endian::Big);
        assert_eq!(m["metadata_size"], 768);
        assert_eq!(m["bitmask_1"], 5);
        assert_eq!(m["high_tension"], 300_000.0);
        assert_eq!(m["camera_name"], "Falcon");
        assert!(!m.contains_key("scan_rotation"));
        assert_eq!(fei_block_len(&b, Endian::Big), Some(768));
    }

    #[test]
    fn serialem_records() {
        assert_eq!(seri_bytes(1 | 4 | 8), Some(8));
        let mut r = Vec::new();
        r.extend_from_slice(&(-4550i16).to_le_bytes()); // tilt -45.5
        r.extend_from_slice(&(250i16).to_le_bytes()); // stage x 10 µm
        r.extend_from_slice(&(-50i16).to_le_bytes()); // stage y -2 µm
        r.extend_from_slice(&(290i16).to_le_bytes()); // mag 29000
        let m = seri_record(&r, 1 | 4 | 8, Endian::Little);
        assert_eq!(m["tilt_angle_deg"], -45.5);
        assert_eq!(m["stage_x_um"], 10.0);
        assert_eq!(m["stage_y_um"], -2.0);
        assert_eq!(m["magnification"], 29_000.0);
        // 1.5 = (1*256 + 128) * 2^-8
        assert!((seri_float(1, -(8 * 256 + 128)) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn symmetry_text() {
        let mut b = vec![b' '; 160];
        b[..11].copy_from_slice(b"X,Y,Z * -X,");
        assert_eq!(symmetry_lines(&b), vec!["X,Y,Z * -X,".to_string()]);
        assert!(is_text(&b));
    }
}
