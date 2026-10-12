//! Thermo Fisher OMNIC map files (`.map`, OMNIC Atlµs): the spectra of a `.spa`-style or
//! `.srs`-style file plus the stage position of each spectrum (key 6) and a video image of the
//! sample (key 147).
//!
//! Layout and vocabulary: `docs/formats/thermo-omnic.md`; provenance:
//! `docs/provenance/thermo-omnic.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::Finding;
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::OMNIC_FORMAT_ID as FMT;
use crate::common::{
    Le, MapSpec, Parsed, RasterLayout, RasterSpec, SpectrumTable, Stored, num, read_at,
};

/// Key-6 kind: a list of (x, y) stage positions in µm.
const POINT_LIST: u16 = 2;
/// Key-6 kind: one rectangle (x0, y0, x1, y1) in µm.
const RECTANGLE: u16 = 4;
/// Most positions read.
const MAX_POINTS: usize = 1 << 22;
/// Positions closer than this (µm) are one grid line.
const SAME_LINE_UM: f64 = 0.01;

/// The map records of a file, gathered from its key table.
#[derive(Debug, Default)]
pub(crate) struct MapRecords {
    /// The stage position of each spectrum, µm.
    pub(crate) points: Option<Vec<(f64, f64)>>,
    /// The rectangle stored with the image, µm.
    pub(crate) rectangle: Option<[f64; 4]>,
    /// Offset and length of the image record (key 147).
    pub(crate) image: Option<(u64, u64)>,
}

impl MapRecords {
    /// Read one key-6 record (u16 count, u16 kind, two bytes, then float32 values).
    pub(crate) fn shape(&mut self, b: &[u8]) {
        let (Some(count), Some(kind)) = (b.u16_at(0), b.u16_at(2)) else {
            return;
        };
        let count = usize::from(count);
        let floats = |n: usize| -> Option<Vec<f64>> {
            (0..n)
                .map(|i| b.f32_at(6 + 4 * i).map(f64::from))
                .collect::<Option<Vec<f64>>>()
        };
        match kind {
            POINT_LIST if self.points.is_none() && count <= MAX_POINTS => {
                if let Some(v) = floats(2 * count) {
                    self.points = Some(v.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect());
                }
            }
            RECTANGLE if self.rectangle.is_none() && count == 1 => {
                if let Some(v) = floats(4) {
                    self.rectangle = Some([v[0], v[1], v[2], v[3]]);
                }
            }
            _ => {}
        }
    }

    /// The positions, which must name one spectrum each (else the map is refused).
    pub(crate) fn checked_points(&self, spectra: u64) -> Result<&[(f64, f64)]> {
        let Some(p) = &self.points else {
            return Err(Error::unsupported(
                FMT,
                "an OMNIC map file without a list of stage positions",
                "export the map's spectra from OMNIC (File > Save As, .spa or .csv)",
            ));
        };
        if p.len() as u64 != spectra {
            return Err(Error::unsupported(
                FMT,
                format!(
                    "an OMNIC map file with {} stage positions for {spectra} spectra",
                    p.len()
                ),
                "export the map's spectra from OMNIC (File > Save As, .spa or .csv)",
            ));
        }
        Ok(p)
    }
}

/// Sorted distinct values (closer than [`SAME_LINE_UM`] count as one).
fn lines(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = values.filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < SAME_LINE_UM);
    v
}

/// The step of evenly spaced lines (within 1 %), if they are.
fn even_step(lines: &[f64]) -> Option<f64> {
    if lines.len() < 2 {
        return None;
    }
    let step = (lines[lines.len() - 1] - lines[0]) / (lines.len() - 1) as f64;
    (step > 0.0
        && lines
            .windows(2)
            .all(|w| ((w[1] - w[0]) - step).abs() <= 0.01 * step))
    .then_some(step)
}

/// The x lines, the y lines, and each position's (column, row).
type Grid = (Vec<f64>, Vec<f64>, Vec<(u32, u32)>);

/// Column and row of each position when the positions fill a grid of evenly spaced lines, one
/// spectrum per node.
fn grid(points: &[(f64, f64)]) -> Option<Grid> {
    let xs = lines(points.iter().map(|p| p.0));
    let ys = lines(points.iter().map(|p| p.1));
    if xs.len().checked_mul(ys.len())? != points.len() || points.len() < 2 {
        return None;
    }
    if (xs.len() > 1 && even_step(&xs).is_none()) || (ys.len() > 1 && even_step(&ys).is_none()) {
        return None;
    }
    let find = |lines: &[f64], v: f64| {
        lines
            .iter()
            .position(|l| (l - v).abs() < SAME_LINE_UM)
            .and_then(|i| u32::try_from(i).ok())
    };
    let mut seen = vec![false; points.len()];
    let mut cells = Vec::with_capacity(points.len());
    for p in points {
        let (i, j) = (find(&xs, p.0)?, find(&ys, p.1)?);
        let k = (j as usize)
            .checked_mul(xs.len())?
            .checked_add(i as usize)?;
        if std::mem::replace(seen.get_mut(k)?, true) {
            return None;
        }
        cells.push((i, j));
    }
    Some((xs, ys, cells))
}

/// Add the map's positions table (for set `set`), its grid image when the positions form a
/// grid, and the video image.
pub(crate) fn add(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    parsed: &mut Parsed,
    set: usize,
    records: &MapRecords,
    points: &[(f64, f64)],
) -> Result<()> {
    let trace = u32::try_from(set).unwrap_or(u32::MAX);
    let mut columns = vec![
        (
            "spectrum".to_string(),
            None,
            (0..points.len()).map(|k| k as f64).collect(),
        ),
        (
            "x_um".to_string(),
            Some("µm".to_string()),
            points.iter().map(|p| p.0).collect(),
        ),
        (
            "y_um".to_string(),
            Some("µm".to_string()),
            points.iter().map(|p| p.1).collect(),
        ),
    ];
    let mut note = format!(
        "OMNIC map file (.map): {} spectra at the stage positions in table `positions`",
        points.len()
    );
    if let Some((xs, ys, cells)) = grid(points) {
        columns.push((
            "column".into(),
            None,
            cells.iter().map(|c| f64::from(c.0)).collect(),
        ));
        columns.push((
            "row".into(),
            None,
            cells.iter().map(|c| f64::from(c.1)).collect(),
        ));
        let (w, h) = (
            u32::try_from(xs.len()).unwrap_or(u32::MAX),
            u32::try_from(ys.len()).unwrap_or(u32::MAX),
        );
        let mut pixels = vec![None; xs.len() * ys.len()];
        for (k, (i, j)) in cells.iter().enumerate() {
            let at = *j as usize * xs.len() + *i as usize;
            if let Some(px) = pixels.get_mut(at) {
                *px = u32::try_from(k).ok();
            }
        }
        let mut extra = BTreeMap::new();
        extra.insert(
            "row_order".into(),
            json!("row 0 is the smallest y_um, column 0 the smallest x_um"),
        );
        extra.insert("origin_um".into(), json!([num(xs[0]), num(ys[0])]));
        parsed.maps.push(MapSpec {
            name: "map".into(),
            set,
            width: w,
            height: h,
            pixels,
            pixel_um: (even_step(&xs), even_step(&ys)),
            extra,
        });
        note.push_str(&format!(", a grid of {w} × {h}"));
        parsed
            .provenance
            .insert("images[].physical_size".into(), Source::Inferred);
    }
    parsed.tables.push(SpectrumTable {
        name: "positions".into(),
        trace,
        columns,
    });
    parsed.notes.push(note);
    parsed
        .provenance
        .insert("tables[].columns".into(), Source::Inferred);
    if let Some((offset, length)) = records.image {
        video_image(f, path, file_len, parsed, records.rectangle, offset, length)?;
    }
    Ok(())
}

/// The video image (key 147): a u32 byte count, then a Windows BMP.
fn video_image(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    parsed: &mut Parsed,
    rectangle: Option<[f64; 4]>,
    offset: u64,
    length: u64,
) -> Result<()> {
    let hdr = read_at(f, path, offset, length.min(58), file_len)?;
    let bmp = hdr.get(4..).unwrap_or_default();
    let (Some(size), Some(data), Some(w), Some(h), Some(bpp), Some(compression)) = (
        hdr.u32_at(0),
        bmp.u32_at(10),
        bmp.i32_at(18),
        bmp.i32_at(22),
        bmp.u16_at(28),
        bmp.u32_at(30),
    ) else {
        parsed
            .notes
            .push("the map's video image record is too short to read".into());
        return Ok(());
    };
    if !bmp.starts_with(b"BM") || bpp != 24 || compression != 0 || w <= 0 || h <= 0 {
        parsed
            .notes
            .push("the map's video image is not an uncompressed 24-bit BMP; it is not read".into());
        return Ok(());
    }
    let mut extra = BTreeMap::new();
    extra.insert("kind".into(), json!("video image"));
    if let Some(r) = rectangle {
        extra.insert(
            "stage_rectangle_um".into(),
            json!([num(r[0]), num(r[1]), num(r[2]), num(r[3])]),
        );
    }
    let r = RasterSpec {
        name: "video image".into(),
        width: w.unsigned_abs(),
        height: h.unsigned_abs(),
        stored: Stored::U8,
        components: 3,
        layout: RasterLayout::Bmp24BottomUp,
        offset: offset + 4 + u64::from(data),
        invalid_rows: Vec::new(),
        pixel_um: (None, None),
        extra,
    };
    let fits = r
        .stored_bytes()
        .and_then(|n| n.checked_add(u64::from(data)))
        .is_some_and(|end| end <= u64::from(size) && end <= length.saturating_sub(4));
    if fits {
        parsed.rasters.push(r);
    } else {
        parsed.findings.push(Finding::error(
            "bitmap_size",
            "the map's video image is shorter than its BMP header says",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape_bytes(kind: u16, values: &[f32]) -> Vec<u8> {
        let count = if kind == POINT_LIST {
            values.len() / 2
        } else {
            1
        };
        let mut b = u16::try_from(count).unwrap().to_le_bytes().to_vec();
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        for v in values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    #[test]
    fn grids_and_point_lists() {
        let mut m = MapRecords::default();
        // 3 × 2 grid, x fastest
        let pts = [
            83.0, -10.0, 88.0, -10.0, 93.0, -10.0, 83.0, -5.0, 88.0, -5.0, 93.0, -5.0,
        ];
        m.shape(&shape_bytes(POINT_LIST, &pts));
        m.shape(&shape_bytes(RECTANGLE, &[0.0, -20.0, 100.0, 0.0]));
        let p = m.checked_points(6).unwrap();
        let (xs, ys, cells) = grid(p).unwrap();
        assert_eq!(xs.len(), 3);
        assert_eq!(ys.len(), 2);
        assert_eq!(cells[4], (1, 1));
        assert_eq!(even_step(&xs), Some(5.0));
        assert_eq!(m.rectangle, Some([0.0, -20.0, 100.0, 0.0]));
        assert_eq!(m.checked_points(5).unwrap_err().exit_code(), 6);
        // a free list of points is not a grid
        assert!(grid(&[(0.0, 0.0), (1.0, 0.3), (2.5, 0.7)]).is_none());
        // truncated records are ignored
        let b = shape_bytes(POINT_LIST, &pts);
        for cut in 0..b.len() {
            let mut m = MapRecords::default();
            m.shape(&b[..cut]);
            assert!(m.points.is_none());
        }
        assert_eq!(
            MapRecords::default()
                .checked_points(1)
                .unwrap_err()
                .exit_code(),
            6
        );
    }
}
