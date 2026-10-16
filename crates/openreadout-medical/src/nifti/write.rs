//! NIfTI-1 writer for `export --format nifti`: one image as a single `.nii` (or `.nii.gz`)
//! file, with its voxel sizes and, when the source records its orientation, a scanner-space
//! affine (sform and qform). Written to a temporary file, read back with this crate's reader
//! and compared plane by plane, then renamed into place.

use std::io::Write;
use std::path::Path;

use openreadout_core::model::ImageInfo;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, PixelType, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::NiftiReader;

/// Options of [`export_nifti`].
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NiftiExportOptions {
    /// The image to write (default 0).
    pub image: Option<u32>,
    /// Replace an existing output.
    pub overwrite: bool,
}

/// What [`export_nifti`] reports.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NiftiExportReport {
    /// The file that was read.
    pub input: String,
    /// The NIfTI file written.
    pub output: String,
    /// Always `nifti`.
    pub format: String,
    /// The image written.
    pub image: u32,
    /// `dim[1..]` written: x, y, z, t (and channels as the fifth dimension).
    pub dims: Vec<u32>,
    /// The NIfTI data type written (`int16`, `float32`, `rgb24`, ...).
    pub datatype: String,
    /// Voxel size in mm (x, y, z) and the time step in seconds, as written to `pixdim`.
    pub pixdim: Vec<f64>,
    /// Where the affine came from: `dicom` (ImagePositionPatient and ImageOrientationPatient),
    /// `nifti` (the source's own), or `none` (voxel sizes only).
    pub affine_from: String,
    /// Planes written.
    pub planes_written: u64,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the file was read back and every plane hashed equal to the source plane.
    pub verified: bool,
    /// What the export assumed or left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The output name `export` uses by default: `<stem>.nii.gz` next to `base`.
pub fn default_nifti_output(base: &Path) -> std::path::PathBuf {
    let name = base
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().to_string());
    let stem = name
        .strip_suffix(".gz")
        .unwrap_or(&name)
        .rsplit_once('.')
        .map_or(name.as_str(), |(s, _)| s)
        .to_string();
    base.with_file_name(format!("{stem}.nii.gz"))
}

fn datatype(pt: PixelType, spp: u32) -> Result<(i16, i16, &'static str)> {
    Ok(match (pt, spp) {
        (PixelType::Uint8, 1) => (2, 8, "uint8"),
        (PixelType::Uint8, 3) => (128, 24, "rgb24"),
        (PixelType::Uint8, 4) => (2304, 32, "rgba32"),
        (PixelType::Int8, 1) => (256, 8, "int8"),
        (PixelType::Int16, 1) => (4, 16, "int16"),
        (PixelType::Uint16, 1) => (512, 16, "uint16"),
        (PixelType::Int32, 1) => (8, 32, "int32"),
        (PixelType::Uint32, 1) => (768, 32, "uint32"),
        (PixelType::Int64, 1) => (1024, 64, "int64"),
        (PixelType::Uint64, 1) => (1280, 64, "uint64"),
        (PixelType::Float, 1) => (16, 32, "float32"),
        (PixelType::Double, 1) => (64, 64, "float64"),
        (PixelType::ComplexFloat, 1) => (32, 64, "complex64"),
        (PixelType::ComplexDouble, 1) => (1792, 128, "complex128"),
        (pt, spp) => {
            return Err(Error::unsupported(
                "nifti",
                format!("{spp} samples per pixel of {}", pt.ome_name()),
                "NIfTI stores one sample per voxel, or 8-bit RGB / RGBA; export --format ome-tiff instead.",
            ));
        }
    })
}

fn f3(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = v?.as_array()?;
    (a.len() >= 3).then(|| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0)])
}

/// The voxel-to-RAS affine of `im` (mm) and where it came from.
fn affine(im: &ImageInfo, dx: f64, dy: f64, dz: f64) -> Option<([[f64; 4]; 3], &'static str)> {
    if let Some(rows) = im.extra.get("affine").and_then(Value::as_array)
        && rows.len() >= 3
    {
        let mut m = [[0.0; 4]; 3];
        for (i, r) in rows.iter().take(3).enumerate() {
            let r = r.as_array()?;
            for j in 0..4 {
                m[i][j] = r.get(j)?.as_f64()?;
            }
        }
        return Some((m, "nifti"));
    }
    let o = im.extra.get("orientation")?;
    let row = f3(o.get("row_direction"))?;
    let col = f3(o.get("column_direction"))?;
    let pos = f3(o.get("first_position_mm"))?;
    let n = crate::util::cross(row, col);
    // LPS (DICOM) to RAS (NIfTI): negate x and y
    let s = [-1.0, -1.0, 1.0];
    let mut m = [[0.0; 4]; 3];
    for a in 0..3 {
        m[a][0] = s[a] * row[a] * dx;
        m[a][1] = s[a] * col[a] * dy;
        m[a][2] = s[a] * n[a] * dz;
        m[a][3] = s[a] * pos[a];
    }
    Some((m, "dicom"))
}

/// A quaternion representation (b, c, d, qfac) of the rotation part of `m` (columns scaled by
/// `d`), following the method of `nifti1.h`.
fn quaternion(m: &[[f64; 4]; 3], d: [f64; 3]) -> ([f64; 3], f64) {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = if d[j] != 0.0 { m[i][j] / d[j] } else { 0.0 };
        }
    }
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    let qfac = if det < 0.0 { -1.0 } else { 1.0 };
    if qfac < 0.0 {
        for row in &mut r {
            row[2] = -row[2];
        }
    }
    let a = 1.0 + r[0][0] + r[1][1] + r[2][2];
    let (a, b, c, dd) = if a > 0.5 {
        let a = 0.5 * a.sqrt();
        (a, 0.25 * (r[2][1] - r[1][2]) / a, 0.25 * (r[0][2] - r[2][0]) / a, 0.25 * (r[1][0] - r[0][1]) / a)
    } else {
        let xd = 1.0 + r[0][0] - (r[1][1] + r[2][2]);
        let yd = 1.0 + r[1][1] - (r[0][0] + r[2][2]);
        let zd = 1.0 + r[2][2] - (r[0][0] + r[1][1]);
        if xd > 1.0 {
            let b = 0.5 * xd.sqrt();
            (0.25 * (r[2][1] - r[1][2]) / b, b, 0.25 * (r[0][1] + r[1][0]) / b, 0.25 * (r[0][2] + r[2][0]) / b)
        } else if yd > 1.0 {
            let c = 0.5 * yd.sqrt();
            (0.25 * (r[0][2] - r[2][0]) / c, 0.25 * (r[0][1] + r[1][0]) / c, c, 0.25 * (r[1][2] + r[2][1]) / c)
        } else {
            let dq = 0.5 * zd.sqrt();
            (0.25 * (r[1][0] - r[0][1]) / dq, 0.25 * (r[0][2] + r[2][0]) / dq, 0.25 * (r[1][2] + r[2][1]) / dq, dq)
        }
    };
    let (b, c, dd) = if a < 0.0 { (-b, -c, -dd) } else { (b, c, dd) };
    ([b, c, dd], qfac)
}

/// The 348-byte NIfTI-1 header plus the 4-byte extension flag.
#[allow(clippy::too_many_arguments)]
fn header(
    dims: &[u32],
    dt: (i16, i16),
    pixdim: [f64; 5],
    aff: Option<[[f64; 4]; 3]>,
    window: Option<(f64, f64)>,
    descrip: &str,
) -> Vec<u8> {
    let mut h = vec![0u8; 352];
    let put_i16 = |h: &mut [u8], o: usize, v: i16| h[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let put_f32 = |h: &mut [u8], o: usize, v: f64| h[o..o + 4].copy_from_slice(&(v as f32).to_le_bytes());
    h[0..4].copy_from_slice(&348i32.to_le_bytes());
    put_i16(&mut h, 40, dims.len() as i16);
    for (i, d) in dims.iter().enumerate() {
        put_i16(&mut h, 42 + 2 * i, (*d).min(i16::MAX as u32) as i16);
    }
    for i in dims.len() + 1..8 {
        put_i16(&mut h, 40 + 2 * i, 1);
    }
    put_i16(&mut h, 70, dt.0);
    put_i16(&mut h, 72, dt.1);
    let (q, qfac) = aff.map_or(([0.0; 3], 1.0), |m| quaternion(&m, [pixdim[1], pixdim[2], pixdim[3]]));
    put_f32(&mut h, 76, qfac);
    for (i, v) in pixdim.iter().enumerate().skip(1) {
        put_f32(&mut h, 76 + 4 * i, *v);
    }
    put_f32(&mut h, 108, 352.0);
    // values are written as they are returned (already rescaled): no scaling
    put_f32(&mut h, 112, 0.0);
    h[123] = 2 | 8; // mm and seconds
    if let Some((c, w)) = window {
        put_f32(&mut h, 124, c + w / 2.0);
        put_f32(&mut h, 128, c - w / 2.0);
    }
    let d = descrip.as_bytes();
    h[148..148 + d.len().min(79)].copy_from_slice(&d[..d.len().min(79)]);
    if let Some(m) = aff {
        put_i16(&mut h, 252, 1); // qform: scanner anatomical
        put_i16(&mut h, 254, 1); // sform: scanner anatomical
        put_f32(&mut h, 256, q[0]);
        put_f32(&mut h, 260, q[1]);
        put_f32(&mut h, 264, q[2]);
        put_f32(&mut h, 268, m[0][3]);
        put_f32(&mut h, 272, m[1][3]);
        put_f32(&mut h, 276, m[2][3]);
        for (k, row) in m.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                put_f32(&mut h, 280 + 16 * k + 4 * j, *v);
            }
        }
    }
    h[344..348].copy_from_slice(b"n+1\0");
    h
}

/// Write image `opts.image` of `ds` to `output` as NIfTI-1 (`.nii`, or gzip-compressed when the
/// name ends in `.gz`), read it back and compare every plane, then rename it into place.
pub fn export_nifti(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &NiftiExportOptions,
) -> Result<NiftiExportReport> {
    openreadout_core::output::refuse_dataset(output, input, &*ds)?;
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let info = ds.info()?;
    let index = opts.image.unwrap_or(0);
    let im = info
        .images
        .iter()
        .find(|i| i.index == index)
        .cloned()
        .ok_or_else(|| {
            Error::Usage(format!(
                "image {index} not found ({} images; `openreadout info` lists them)",
                info.images.len()
            ))
        })?;
    if im.pyramid_levels > 1 {
        return Err(Error::unsupported(
            "nifti",
            "NIfTI export of a whole-slide pyramid",
            "NIfTI holds volumes; export slides to OME-TIFF or OME-Zarr (--format ome-tiff).",
        ));
    }
    let (code, bitpix, dt_name) = datatype(im.pixel_type, im.samples_per_pixel)?;
    if [im.size_x, im.size_y, im.size_z, im.size_t, im.size_c]
        .iter()
        .any(|v| *v > i16::MAX as u32)
    {
        return Err(Error::unsupported(
            "nifti",
            "an image with a side above 32767",
            "NIfTI-1 dimensions are 16-bit; export to OME-TIFF or OME-Zarr instead.",
        ));
    }
    let mut notes = Vec::new();
    let mm = |v: Option<f64>| v.map_or(1.0, |um| um / 1000.0);
    let (dx, dy, dz) = (mm(im.physical_size.x), mm(im.physical_size.y), mm(im.physical_size.z));
    if im.physical_size.x.is_none() {
        notes.push("the source records no pixel size: pixdim is 1 mm".into());
    }
    let dt_s = im.time_increment_s.unwrap_or(1.0);
    let mut dims = vec![im.size_x, im.size_y, im.size_z, im.size_t, im.size_c];
    while dims.len() > 3 && *dims.last().unwrap_or(&1) == 1 {
        dims.pop();
    }
    let aff = affine(&im, dx, dy, dz);
    if aff.is_none() {
        notes.push("the source records no orientation: qform and sform are not set".into());
    }
    let window = im
        .extra
        .get("windows")
        .and_then(Value::as_array)
        .and_then(|w| w.first())
        .and_then(|w| Some((w.get("center")?.as_f64()?, w.get("width")?.as_f64()?)));
    let descrip = format!(
        "openreadout {} from {}",
        env!("CARGO_PKG_VERSION"),
        info.format.id
    );
    let hdr = header(
        &dims,
        (code, bitpix),
        [0.0, dx, dy, dz, dt_s],
        aff.map(|a| a.0),
        window,
        &descrip,
    );
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let gz = output
        .file_name()
        .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".gz"));
    let tmp = openreadout_core::output::temp_path(output);
    let result = (|| -> Result<(u64, Vec<String>)> {
        let file = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let buffered = std::io::BufWriter::new(file);
        let mut w: Box<dyn Write> = if gz {
            Box::new(flate2::write::GzEncoder::new(buffered, flate2::Compression::default()))
        } else {
            Box::new(buffered)
        };
        let io = |e| Error::io(&tmp, e);
        w.write_all(&hdr).map_err(io)?;
        let mut hashes = Vec::new();
        for c in 0..im.size_c {
            for t in 0..im.size_t {
                for z in 0..im.size_z {
                    let p = ds.read_plane(index, PlaneIndex { c, z, t })?;
                    hashes.push(p.xxh3_hex());
                    w.write_all(&p.data).map_err(io)?;
                }
            }
        }
        w.flush().map_err(io)?;
        drop(w);
        // read back through the reader
        let mut back = NiftiReader.open_input(&Input::local(&tmp))?;
        let mut i = 0usize;
        for c in 0..im.size_c {
            for t in 0..im.size_t {
                for z in 0..im.size_z {
                    let p = back.read_plane(0, PlaneIndex { c, z, t })?;
                    if p.xxh3_hex() != hashes[i] {
                        return Err(Error::Other(format!(
                            "NIfTI verification failed: plane c={c} z={z} t={t} differs after writing"
                        )));
                    }
                    i += 1;
                }
            }
        }
        let len = std::fs::metadata(&tmp).map_err(|e| Error::io(&tmp, e))?.len();
        Ok((len, hashes))
    })();
    let (bytes, hashes) = match result {
        Ok(v) => v,
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            return Err(e);
        }
    };
    if output.exists() {
        std::fs::remove_file(output).map_err(|e| Error::io(output, e))?;
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(NiftiExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "nifti".into(),
        image: index,
        dims,
        datatype: dt_name.into(),
        pixdim: vec![dx, dy, dz, dt_s].into_iter().map(openreadout_core::model::round_noise).collect(),
        affine_from: aff.map_or("none", |a| a.1).into(),
        planes_written: hashes.len() as u64,
        bytes_written: bytes,
        verified: true,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rotation_has_a_zero_quaternion() {
        let m = [[2.0, 0.0, 0.0, 1.0], [0.0, 2.0, 0.0, 2.0], [0.0, 0.0, 3.0, 3.0]];
        let (q, qfac) = quaternion(&m, [2.0, 2.0, 3.0]);
        assert!(q.iter().all(|v| v.abs() < 1e-12));
        assert_eq!(qfac, 1.0);
        // flipped x and y (DICOM LPS axial to RAS): a 180° turn about z
        let m = [[-1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let (q, _) = quaternion(&m, [1.0, 1.0, 1.0]);
        assert!((q[2].abs() - 1.0).abs() < 1e-12);
    }
}
