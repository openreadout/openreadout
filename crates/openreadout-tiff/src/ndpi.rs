//! Tiled access to the full-resolution page of a Hamamatsu NDPI file.
//!
//! NDPI stores the full-resolution image of a slide as one baseline JPEG (a single strip of
//! tens of thousands of pixels in each direction) with restart markers, and lists the byte
//! offset of every restart interval, relative to the strip, in private tag 65426 (with the high
//! 32 bits of offsets beyond 4 GiB in tag 65432). Each interval is independently decodable: the
//! DC predictors restart at every marker. An interval covers `restart interval × MCU width`
//! pixels of one MCU row, so the page can be read as a grid of such "tiles": a tile is decoded
//! as a small JPEG made of the page's header (SOF dimensions set to the tile's, the restart
//! interval removed), the interval's entropy-coded bytes and an end marker.
//!
//! See `docs/formats/tiff.md` § NDPI and `docs/provenance/tiff.md` (prior art: tifffile, BSD-3).

use std::sync::Arc;

use openreadout_core::bytes::be_u16;
use openreadout_core::model::{InstrumentInfo, ObjectiveInfo};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::container::ByteSource;
use crate::dataset::{Attachment, Level, TiffDataset, text_value, tiff_datetime};
use crate::decode::PageLayout;
use crate::{FORMAT_ID, tags};

/// NDPI: offsets of the restart intervals of the full-resolution JPEG, relative to the strip.
pub(crate) const MCU_STARTS: u16 = 65426;
/// NDPI: high 32 bits of those offsets (files above 4 GiB).
pub(crate) const MCU_STARTS_HIGH: u16 = 65432;

/// Most header bytes read before the start of the scan.
const MAX_HEADER: u64 = 1 << 20;

/// The JPEG header of the page, rewritten for one tile, and the tile geometry.
#[derive(Debug, Clone)]
pub(crate) struct NdpiHeader {
    /// SOI .. SOS (inclusive) with the frame size set to one tile and no restart interval.
    pub header: Vec<u8>,
    pub tile_width: u32,
    pub tile_height: u32,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, format!("NDPI JPEG header: {}", detail.into()))
}

/// Parse the page's JPEG header (`bytes` = the strip from its start up to the first restart
/// interval) and rewrite it for one tile.
pub(crate) fn parse_header(bytes: &[u8]) -> Result<NdpiHeader> {
    if bytes.get(..2) != Some(&[0xFF, 0xD8][..]) {
        return Err(bad("the strip does not start with a JPEG SOI marker"));
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2usize;
    let (mut sof_at, mut mcu_w, mut mcu_h, mut restart) = (None, 0u32, 0u32, 0u32);
    loop {
        let (Some(&ff), Some(&m)) = (bytes.get(i), bytes.get(i + 1)) else {
            return Err(bad("no start-of-scan marker"));
        };
        if ff != 0xFF {
            return Err(bad(format!("expected a marker at byte {i}")));
        }
        let len = be_u16(bytes, i + 2)
            .map(usize::from)
            .ok_or_else(|| bad("truncated segment"))?;
        let seg = bytes
            .get(i..i + 2 + len)
            .ok_or_else(|| bad("truncated segment"))?;
        match m {
            // Baseline and extended sequential DCT frames.
            0xC0 | 0xC1 => {
                let nf = usize::from(*seg.get(9).ok_or_else(|| bad("short SOF"))?);
                let (mut hmax, mut vmax) = (1u32, 1u32);
                for k in 0..nf {
                    let hv = *seg.get(11 + 3 * k).ok_or_else(|| bad("short SOF"))?;
                    hmax = hmax.max(u32::from(hv >> 4));
                    vmax = vmax.max(u32::from(hv & 15));
                }
                mcu_w = 8 * hmax;
                mcu_h = 8 * vmax;
                sof_at = Some(out.len());
                out.extend_from_slice(seg);
            }
            0xC2..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "NDPI full-resolution JPEG that is not baseline/sequential",
                    "Read a reduced-resolution level (--level 1 or higher) instead.",
                ));
            }
            // Restart interval: kept out of the per-tile header (a tile is one interval).
            0xDD => {
                restart = u32::from(be_u16(seg, 4).ok_or_else(|| bad("short DRI"))?);
            }
            0xDA => {
                out.extend_from_slice(seg);
                break;
            }
            _ => out.extend_from_slice(seg),
        }
        i += 2 + len;
    }
    let sof = sof_at.ok_or_else(|| bad("no frame header (SOF)"))?;
    if restart == 0 || mcu_w == 0 {
        return Err(bad("no restart interval: the page cannot be read by tiles"));
    }
    let tile_width = restart * mcu_w;
    // Frame size: height at +5, width at +7 of the SOF segment.
    out[sof + 5..sof + 7].copy_from_slice(&u16::try_from(mcu_h).unwrap_or(u16::MAX).to_be_bytes());
    out[sof + 7..sof + 9]
        .copy_from_slice(&u16::try_from(tile_width).unwrap_or(u16::MAX).to_be_bytes());
    Ok(NdpiHeader {
        header: out,
        tile_width,
        tile_height: mcu_h,
    })
}

/// The page layout as a grid of restart-interval tiles, when `layout` is a single-strip JPEG
/// page with NDPI restart offsets (`starts`, relative to the strip). The tiles' `jpeg_frame`
/// is the rewritten header.
pub(crate) fn tiled_layout(
    src: &mut ByteSource,
    layout: &PageLayout,
    starts: &[u64],
) -> Result<PageLayout> {
    let (Some(&strip), Some(&count)) = (layout.offsets.first(), layout.byte_counts.first()) else {
        return Err(bad("the page has no strip"));
    };
    let first = *starts.first().ok_or_else(|| bad("no restart offsets"))?;
    if first > MAX_HEADER || first > count {
        return Err(bad(format!(
            "first restart interval at {first}, beyond the header"
        )));
    }
    let head = src.read_at(strip, first)?;
    let h = parse_header(&head)?;
    let across = layout.width.div_ceil(h.tile_width);
    let down = layout.height.div_ceil(h.tile_height);
    if u64::from(across) * u64::from(down) != starts.len() as u64 {
        return Err(bad(format!(
            "{} restart offsets for a {across} x {down} grid of {}x{} intervals",
            starts.len(),
            h.tile_width,
            h.tile_height
        )));
    }
    let mut offsets = Vec::with_capacity(starts.len());
    let mut counts = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(count);
        if end <= s || end > count {
            return Err(bad(format!(
                "restart offset {k} ({s}) is out of order or past the strip"
            )));
        }
        offsets.push(strip + s);
        counts.push(end - s);
    }
    let mut t = layout.clone();
    t.tiled = true;
    t.chunk_width = h.tile_width;
    t.chunk_height = h.tile_height;
    t.offsets = offsets;
    t.byte_counts = counts;
    t.jpeg_tables = None;
    t.jpeg_frame = Some(Arc::new(h.header));
    t.ndpi_mcu_starts = None;
    Ok(t)
}

/// A complete JPEG for one interval: the rewritten header, the interval's bytes without its
/// trailing restart (or end) marker, and an end-of-image marker.
pub(crate) fn frame(header: &[u8], interval: &[u8]) -> Vec<u8> {
    let mut body = interval;
    if let [rest @ .., 0xFF, m] = body
        && ((0xD0..=0xD7).contains(m) || *m == 0xD9)
    {
        body = rest;
    }
    let mut out = Vec::with_capacity(header.len() + body.len() + 2);
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

impl TiffDataset {
    pub(crate) fn build_ndpi(&mut self) -> Result<()> {
        let n = self.main().ifds.len();
        let mut pyramid: Vec<(usize, (u32, u32))> = Vec::new();
        for p in 0..n {
            let mag = self.main().ifds[p]
                .float(tags::NDPI_MAGNIFICATION)
                .unwrap_or(0.0);
            if mag > 0.0 {
                pyramid.push((p, self.page_dims(p)));
            } else {
                self.attachments.push(Attachment {
                    role: if (mag + 1.0).abs() < 1e-9 {
                        "macro"
                    } else if (mag + 2.0).abs() < 1e-9 {
                        "map"
                    } else {
                        "associated"
                    }
                    .into(),
                    page: p,
                });
            }
        }
        let Some(&(b0, base)) = pyramid
            .iter()
            .max_by_key(|(_, d)| u64::from(d.0) * u64::from(d.1))
        else {
            return Err(Error::corrupt(FORMAT_ID, "NDPI file without pyramid pages"));
        };
        let z_pages: Vec<usize> = pyramid
            .iter()
            .filter(|(_, d)| *d == base)
            .map(|(p, _)| *p)
            .collect();
        let mut levels = Vec::new();
        let mut seen = Vec::new();
        for &(p, d) in &pyramid {
            if d != base && !seen.contains(&d) {
                seen.push(d);
                levels.push(Level {
                    width: d.0,
                    height: d.1,
                    page: Some(p),
                    sub_ifd: None,
                    pages: Vec::new(),
                });
            }
        }
        levels.sort_by_key(|l| std::cmp::Reverse(u64::from(l.width) * u64::from(l.height)));
        let page0 = self.main().ifds[b0].clone();
        let mut info = self.push_slide(
            text_value(&page0, tags::NDPI_SLIDE_LABEL),
            &z_pages,
            &[],
            levels,
        )?;
        if let Some(mag) = page0.float(tags::NDPI_MAGNIFICATION) {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: Some(mag),
                ..ObjectiveInfo::default()
            });
        }
        info.instrument = Some(InstrumentInfo {
            manufacturer: text_value(&page0, tags::MAKE).or_else(|| Some("Hamamatsu".into())),
            model: text_value(&page0, tags::MODEL),
            software: text_value(&page0, tags::SOFTWARE),
            ..InstrumentInfo::default()
        });
        info.acquired_at = text_value(&page0, tags::DATE_TIME).and_then(|d| tiff_datetime(&d));
        if let Some(name) = text_value(&page0, tags::NDPI_FLUORESCENCE)
            && let Some(ch) = info.channels.first_mut()
        {
            ch.name = Some(name);
        }
        let mut ndpi = Map::new();
        for (tag, key) in [
            (tags::NDPI_X_OFFSET_NM, "x_offset_from_slide_center_nm"),
            (tags::NDPI_Y_OFFSET_NM, "y_offset_from_slide_center_nm"),
            (tags::NDPI_Z_OFFSET_NM, "z_offset_nm"),
        ] {
            if let Some(v) = page0.float(tag) {
                ndpi.insert(key.into(), json!(v));
            }
        }
        if let Some(s) = text_value(&page0, tags::NDPI_SCANNER_SERIAL) {
            ndpi.insert("scanner_serial".into(), json!(s));
        }
        if z_pages.len() > 1 {
            let zs: Vec<Value> = z_pages
                .iter()
                .map(|&p| json!(self.main().ifds[p].float(tags::NDPI_Z_OFFSET_NM)))
                .collect();
            ndpi.insert("focal_plane_z_offsets_nm".into(), Value::Array(zs));
        }
        info.extra.insert("ndpi".into(), Value::Object(ndpi));
        self.replace_last(info);
        self.set_provenance(&[
            ("images[].physical_size", Source::Spec),
            ("images[].objective.nominal_magnification", Source::PriorArt),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
            ("images[].channels[].name", Source::PriorArt),
        ]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn header_is_rewritten_for_one_interval() {
        let mut h = vec![0xFF, 0xD8];
        h.extend(seg(0xDB, &[0; 65]));
        // SOF0: precision 8, 38144 x 51200, 3 components 1x1.
        let mut sof = vec![8];
        sof.extend_from_slice(&38144u16.to_be_bytes());
        sof.extend_from_slice(&51200u16.to_be_bytes());
        sof.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
        h.extend(seg(0xC0, &sof));
        h.extend(seg(0xDD, &256u16.to_be_bytes()));
        h.extend(seg(0xDA, &[3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0]));
        let p = parse_header(&h).unwrap();
        assert_eq!((p.tile_width, p.tile_height), (2048, 8));
        assert!(
            !p.header.windows(2).any(|w| w == [0xFF, 0xDD]),
            "DRI removed"
        );
        let at = p.header.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        assert_eq!(&p.header[at + 5..at + 9], &[0, 8, 0x08, 0x00]);
        // Framing drops the trailing restart marker.
        let f = frame(&p.header, &[1, 2, 0xFF, 0xD3]);
        assert_eq!(&f[f.len() - 4..], &[1, 2, 0xFF, 0xD9]);
        // Progressive frames and missing SOS are refused cleanly.
        let mut prog = h.clone();
        let i = prog.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        prog[i + 1] = 0xC2;
        assert_eq!(parse_header(&prog).unwrap_err().exit_code(), 6);
        assert_eq!(parse_header(&h[..h.len() - 14]).unwrap_err().exit_code(), 4);
        assert_eq!(parse_header(b"GIF89a").unwrap_err().exit_code(), 4);
    }
}
