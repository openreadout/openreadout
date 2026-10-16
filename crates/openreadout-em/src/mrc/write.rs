//! MRC2014 export: one image of any reader as an MRC file, sections streamed to a temporary
//! file, read back through our own MRC reader and compared plane by plane, then renamed into
//! place. A movie (time points) becomes an image stack, optionally with every `group` frames
//! summed into one section (the fractions of an electron-event movie); a z stack becomes a
//! volume. See `docs/formats/mrc.md` § Export.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::select::Selection;
use openreadout_core::{Error, PixelType, Plane, Result};
use schemars::JsonSchema;
use serde::Serialize;

use super::{FORMAT_ID, HEADER_LEN, MrcDataset};

/// What to write.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MrcExportOptions {
    /// Image index (default 0).
    pub image: Option<u32>,
    /// Plane selection (`c=0`, `t=0-99`, `z=10-20`).
    pub select: Vec<String>,
    /// Sum every `group` time points into one section (default 1: every frame its own section).
    pub group: Option<u32>,
    /// Replace an existing output.
    pub overwrite: bool,
}

/// What was written.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MrcExportReport {
    /// The file that was read.
    pub input: String,
    /// The MRC file written.
    pub output: String,
    /// `mrc`.
    pub format: String,
    /// Image index exported.
    pub image: u32,
    /// Sections written (NZ).
    pub sections_written: u64,
    /// Source planes (frames) read.
    pub planes_read: u64,
    /// Frames summed per section (1: none).
    pub group: u32,
    /// `image stack` (sections are frames or fractions) or `volume` (sections are z planes).
    pub layout: String,
    /// MRC2014 MODE written: 0 int8, 1 int16, 2 float32, 6 uint16.
    pub mode: u32,
    /// Pixel size written to the header (Å), when the source records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_size_a: Option<f64>,
    /// Size of the output in bytes.
    pub bytes_written: u64,
    /// True when every section was read back and hashed equal to what was written.
    pub verified: bool,
    /// What was done to the values (conversions), if anything.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// MODE and sample type written for a source sample type (`summed`: several frames added).
fn mode_for(pt: PixelType, group: u32) -> (u32, PixelType) {
    let fits_u16 = |max: f64| max * f64::from(group) <= 65_535.0;
    match pt {
        PixelType::Uint8 if fits_u16(255.0) => (6, PixelType::Uint16),
        PixelType::Uint16 if group == 1 => (6, PixelType::Uint16),
        PixelType::Int16 if group == 1 => (1, PixelType::Int16),
        PixelType::Int8 if group == 1 => (0, PixelType::Int8),
        _ => (2, PixelType::Float),
    }
}

/// `p`'s samples converted to `to` (little-endian), with the value range seen.
fn convert(p: &Plane, to: PixelType, range: &mut (f64, f64, f64, f64, u64)) -> Vec<u8> {
    let bps = p.pixel_type.bytes_per_sample();
    let mut out = Vec::with_capacity(p.data.len() / bps * to.bytes_per_sample());
    for c in p.data.chunks_exact(bps) {
        let v = p.pixel_type.sample_f64(c);
        range.0 = range.0.min(v);
        range.1 = range.1.max(v);
        range.2 += v;
        range.3 += v * v;
        range.4 += 1;
        match to {
            PixelType::Uint16 => {
                out.extend_from_slice(&(v.clamp(0.0, 65_535.0) as u16).to_le_bytes());
            }
            PixelType::Int16 => out.extend_from_slice(&(v as i16).to_le_bytes()),
            PixelType::Int8 => {
                out.push(v as i8 as u8);
            }
            _ => out.extend_from_slice(&(v as f32).to_le_bytes()),
        }
    }
    out
}

fn header(
    nx: u32,
    ny: u32,
    nz: u32,
    mode: u32,
    volume: bool,
    pixel_a: Option<f64>,
    stats: (f64, f64, f64, f64),
    label: &str,
) -> Vec<u8> {
    let mut h = vec![0u8; HEADER_LEN];
    let mut put_i = |at: usize, v: i32| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put_i(0, nx as i32);
    put_i(4, ny as i32);
    put_i(8, nz as i32);
    put_i(12, mode as i32);
    put_i(28, nx as i32);
    put_i(32, ny as i32);
    put_i(36, if volume { nz as i32 } else { 1 });
    put_i(64, 1);
    put_i(68, 2);
    put_i(72, 3);
    put_i(88, i32::from(volume));
    put_i(108, 20141);
    put_i(220, 1);
    let mut put_f = |at: usize, v: f64| h[at..at + 4].copy_from_slice(&(v as f32).to_le_bytes());
    let px = pixel_a.unwrap_or(0.0);
    put_f(40, px * f64::from(nx));
    put_f(44, px * f64::from(ny));
    put_f(48, px * f64::from(if volume { nz } else { 1 }));
    put_f(52, 90.0);
    put_f(56, 90.0);
    put_f(60, 90.0);
    put_f(76, stats.0);
    put_f(80, stats.1);
    put_f(84, stats.2);
    put_f(216, stats.3);
    h[208..212].copy_from_slice(b"MAP ");
    h[212..216].copy_from_slice(&[0x44, 0x44, 0, 0]);
    let l = label.as_bytes();
    let n = l.len().min(80);
    h[224..224 + n].copy_from_slice(&l[..n]);
    for b in &mut h[224 + n..304] {
        *b = b' ';
    }
    h
}

/// Write image `opts.image` of `ds` (read from `input`) to `output` as MRC2014.
pub fn export_mrc(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &MrcExportOptions,
) -> Result<MrcExportReport> {
    openreadout_core::output::refuse_inputs(output, [input])?;
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let info = ds.info()?;
    let image = opts.image.unwrap_or(0);
    let im = info
        .images
        .iter()
        .find(|i| i.index == image)
        .ok_or_else(|| {
            Error::Usage(format!(
                "image {image} not found (the file has {})",
                info.images.len()
            ))
        })?
        .clone();
    if im.samples_per_pixel != 1 || im.pixel_type.is_complex() {
        return Err(Error::unsupported(
            "export",
            "MRC export of RGB or complex images",
            "MRC holds one real value per pixel; export colour images to OME-TIFF",
        ));
    }
    let sel = Selection::parse(&opts.select)?;
    let pick = |v: &[u32], size: u32, axis: &str| -> Result<Vec<u32>> {
        for &i in v {
            if i >= size {
                return Err(Error::Usage(format!("{axis}={i} out of range (0..{size})")));
            }
        }
        Ok(if v.is_empty() {
            (0..size).collect()
        } else {
            v.to_vec()
        })
    };
    let cs = pick(&sel.c, im.size_c, "c")?;
    if cs.len() != 1 {
        return Err(Error::Usage(format!(
            "MRC holds one channel; the image has {}: pass a selection such as c=0",
            im.size_c
        )));
    }
    let channel = cs[0];
    let zs = pick(&sel.z, im.size_z, "z")?;
    let ts = pick(&sel.t, im.size_t, "t")?;
    if zs.len() > 1 && ts.len() > 1 {
        return Err(Error::Usage(format!(
            "MRC holds one stack: the image has {} z planes and {} time points; select one z (z=N) or one time point (t=N)",
            zs.len(),
            ts.len()
        )));
    }
    let group = opts.group.unwrap_or(1).max(1);
    let volume = zs.len() > 1;
    if volume && group > 1 {
        return Err(Error::Usage(
            "group sums time points; this export writes z planes (a volume)".into(),
        ));
    }
    // Sections: lists of (z, t) planes added into one.
    let sections: Vec<(u32, Vec<u32>)> = if volume {
        zs.iter().map(|&z| (z, vec![ts[0]])).collect()
    } else {
        ts.chunks(group as usize)
            .map(|g| (zs[0], g.to_vec()))
            .collect()
    };
    let (mode, out_type) = mode_for(im.pixel_type, if group > 1 { group } else { 1 });
    let mut notes = Vec::new();
    if group > 1 {
        notes.push(format!(
            "every {group} frames summed into one section ({} sections; the last holds {})",
            sections.len(),
            sections.last().map_or(0, |s| s.1.len())
        ));
    }
    if out_type != im.pixel_type {
        notes.push(format!(
            "{} samples written as {} (MRC mode {mode})",
            im.pixel_type.ome_name(),
            out_type.ome_name()
        ));
    }
    let pixel_a = im.physical_size.x.filter(|v| *v > 0.0).map(|um| um * 1e4);
    let nz = u32::try_from(sections.len()).map_err(|_| Error::Usage("too many sections".into()))?;
    let label = format!(
        "openreadout {} export of {}",
        env!("CARGO_PKG_VERSION"),
        input
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let tmp = openreadout_core::output::temp_path(output);
    let result = (|| -> Result<MrcExportReport> {
        let mut f =
            std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?);
        f.write_all(&vec![0u8; HEADER_LEN])
            .map_err(|e| Error::io(&tmp, e))?;
        let mut range = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0, 0u64);
        let mut hashes = Vec::with_capacity(sections.len());
        let (mut width, mut height) = (0u32, 0u32);
        let mut read = 0u64;
        for (z, group_ts) in &sections {
            let plane = if group_ts.len() > 1 {
                ds.sum_planes(image, channel, *z, group_ts)?
            } else {
                ds.read_plane(
                    image,
                    PlaneIndex {
                        c: channel,
                        z: *z,
                        t: group_ts[0],
                    },
                )?
            };
            read += group_ts.len() as u64;
            if width == 0 {
                (width, height) = (plane.width, plane.height);
            } else if (plane.width, plane.height) != (width, height) {
                return Err(Error::Other("planes of one image differ in size".into()));
            }
            let written = Plane {
                width,
                height,
                pixel_type: out_type,
                samples_per_pixel: 1,
                data: convert(&plane, out_type, &mut range),
            };
            hashes.push(written.xxh3_hex());
            f.write_all(&written.data).map_err(|e| Error::io(&tmp, e))?;
        }
        let n = range.4.max(1) as f64;
        let mean = range.2 / n;
        let rms = (range.3 / n - mean * mean).max(0.0).sqrt();
        let hdr = header(
            width,
            height,
            nz,
            mode,
            volume,
            pixel_a,
            (range.0, range.1, mean, rms),
            &label,
        );
        let mut file = f
            .into_inner()
            .map_err(|e| Error::io(&tmp, e.into_error()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| Error::io(&tmp, e))?;
        file.write_all(&hdr).map_err(|e| Error::io(&tmp, e))?;
        file.sync_all().map_err(|e| Error::io(&tmp, e))?;
        drop(file);
        // Read back through our own reader and compare every section.
        let mut back = MrcDataset::open(&tmp)?;
        let bi = back.info()?;
        let bim = &bi.images[0];
        let mut k = 0usize;
        for t in 0..bim.size_t {
            for zz in 0..bim.size_z {
                let p = back.read_plane(0, PlaneIndex { c: 0, z: zz, t })?;
                if hashes.get(k) != Some(&p.xxh3_hex()) {
                    return Err(Error::Other(format!(
                        "{}: section {k} read back differs from what was written",
                        tmp.display()
                    )));
                }
                k += 1;
            }
        }
        if k != hashes.len() {
            return Err(Error::Other(format!(
                "{}: {k} sections read back, {} written",
                tmp.display(),
                hashes.len()
            )));
        }
        let bytes_written = std::fs::metadata(&tmp)
            .map_err(|e| Error::io(&tmp, e))?
            .len();
        std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
        Ok(MrcExportReport {
            input: input.display().to_string(),
            output: output.display().to_string(),
            format: FORMAT_ID.into(),
            image,
            sections_written: u64::from(nz),
            planes_read: read,
            group,
            layout: if volume {
                "volume".into()
            } else {
                "image stack".into()
            },
            mode,
            pixel_size_a: pixel_a,
            bytes_written,
            verified: true,
            notes,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `<stem>.mrc` (or `<stem>.mrcs` for a stack of frames) next to `base`.
pub fn default_output(base: &Path, stack: bool) -> std::path::PathBuf {
    let stem = base
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    base.with_file_name(format!("{stem}.{}", if stack { "mrcs" } else { "mrc" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_for_sample_types() {
        assert_eq!(mode_for(PixelType::Uint8, 1), (6, PixelType::Uint16));
        assert_eq!(mode_for(PixelType::Uint8, 40), (6, PixelType::Uint16));
        assert_eq!(mode_for(PixelType::Uint8, 300), (2, PixelType::Float));
        assert_eq!(mode_for(PixelType::Int16, 1), (1, PixelType::Int16));
        assert_eq!(mode_for(PixelType::Int16, 2), (2, PixelType::Float));
        assert_eq!(mode_for(PixelType::Int32, 1), (2, PixelType::Float));
    }
}
