//! Value transforms applied to decoded frames: the Modality LUT (PS3.3 C.11.1: Rescale Slope
//! and Intercept, or a LUT) that turns stored values into output units such as Hounsfield
//! units, and palette colour (C.7.6.3.1.5).

use openreadout_core::{Error, PixelType, Result};

use super::FORMAT_ID;
use super::element::Lookup;
use super::file::DicomFile;
use super::parse::Item;
use super::pixels::{Decoded, PixelDesc};
use super::tags as t;

/// The modality transform of one frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Modality {
    /// Stored values are the output values.
    Identity,
    /// `stored × slope + intercept`.
    Linear { slope: f64, intercept: f64 },
    /// A lookup table: `lut[stored − first]`, clamped to the table.
    Lut {
        first: i64,
        table: Vec<u16>,
        bits: u32,
    },
}

impl Modality {
    /// From Rescale Slope / Intercept (or a Modality LUT Sequence) in `item`.
    pub(crate) fn of(file: &DicomFile, item: &Item) -> Modality {
        if let Some(lut) = item.first(t::MODALITY_LUT_SEQUENCE)
            && let Some(m) = lut_of(file, lut)
        {
            return m;
        }
        linear(
            item.number(t::RESCALE_SLOPE),
            item.number(t::RESCALE_INTERCEPT),
        )
    }

    /// `stored → output` for one value.
    pub(crate) fn map(&self, v: f64) -> f64 {
        match self {
            Modality::Identity => v,
            Modality::Linear { slope, intercept } => v * slope + intercept,
            Modality::Lut { first, table, .. } => {
                let i = (v as i64)
                    .saturating_sub(*first)
                    .clamp(0, table.len() as i64 - 1);
                f64::from(table.get(i as usize).copied().unwrap_or(0))
            }
        }
    }
}

/// The transform for a slope and intercept (absent ones are 1 and 0).
pub(crate) fn linear(slope: Option<f64>, intercept: Option<f64>) -> Modality {
    let slope = slope.filter(|s| s.is_finite() && *s != 0.0).unwrap_or(1.0);
    let intercept = intercept.filter(|i| i.is_finite()).unwrap_or(0.0);
    if slope == 1.0 && intercept == 0.0 {
        Modality::Identity
    } else {
        Modality::Linear { slope, intercept }
    }
}

/// A LUT Descriptor (entries, first mapped value, bits) and LUT Data.
fn lut_of(file: &DicomFile, item: &Item) -> Option<Modality> {
    let d = item.numbers(0x0028_3002);
    if d.len() < 3 {
        return None;
    }
    let entries = if d[0] == 0.0 { 65536 } else { d[0] as usize };
    let first = d[1] as i64;
    let bits = d[2] as u32;
    let data = file.read_value(item.get(0x0028_3006)?).ok()?;
    let table = words(&data, entries, item.get(0x0028_3006)?.little);
    (table.len() == entries).then_some(Modality::Lut { first, table, bits })
}

/// LUT data as entries: one byte or one 16-bit word per entry.
fn words(data: &[u8], entries: usize, little: bool) -> Vec<u16> {
    if data.len() == entries {
        return data.iter().map(|&b| u16::from(b)).collect();
    }
    data.chunks_exact(2)
        .take(entries)
        .map(|c| {
            if little {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect()
}

/// The output sample type of an image whose frames use `transforms`, for stored samples
/// described by `desc`: the stored type when nothing changes the values; for whole-number
/// slopes and intercepts the smallest of int16, uint16 and int32 that holds every rescaled
/// stored value; uint8 or uint16 for LUTs; float32 otherwise.
pub(crate) fn output_type(desc: &PixelDesc, transforms: &[&Modality]) -> PixelType {
    let stored = desc.stored_type();
    if desc.is_colour() || desc.float_bits > 0 {
        return stored;
    }
    if transforms.iter().all(|m| **m == Modality::Identity) {
        return stored;
    }
    if transforms
        .iter()
        .all(|m| matches!(m, Modality::Lut { .. } | Modality::Identity))
    {
        let bits = transforms
            .iter()
            .filter_map(|m| match m {
                Modality::Lut { bits, .. } => Some(*bits),
                _ => None,
            })
            .max()
            .unwrap_or(16);
        let identity_fits = transforms.iter().all(|m| **m != Modality::Identity)
            || matches!(stored, PixelType::Uint8 | PixelType::Uint16);
        if identity_fits {
            return if bits <= 8 && stored == PixelType::Uint8 {
                PixelType::Uint8
            } else {
                PixelType::Uint16
            };
        }
        return PixelType::Float;
    }
    let (lo, hi) = desc.stored_range();
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for m in transforms {
        match m {
            Modality::Identity => {
                min = min.min(lo);
                max = max.max(hi);
            }
            Modality::Linear { slope, intercept } => {
                if slope.fract() != 0.0 || intercept.fract() != 0.0 {
                    return PixelType::Float;
                }
                let (a, b) = (lo * slope + intercept, hi * slope + intercept);
                min = min.min(a.min(b));
                max = max.max(a.max(b));
            }
            Modality::Lut { .. } => return PixelType::Float,
        }
    }
    if min >= f64::from(i16::MIN) && max <= f64::from(i16::MAX) {
        PixelType::Int16
    } else if min >= 0.0 && max <= f64::from(u16::MAX) {
        PixelType::Uint16
    } else if min >= f64::from(i32::MIN) && max <= f64::from(i32::MAX) {
        PixelType::Int32
    } else {
        PixelType::Float
    }
}

/// Apply `m` to a decoded grayscale frame, producing samples of `out`.
pub(crate) fn apply(d: &Decoded, m: &Modality, out: PixelType) -> Vec<u8> {
    if *m == Modality::Identity && d.pixel_type == out {
        return d.data.clone();
    }
    let mut v =
        Vec::with_capacity(d.data.len() / d.pixel_type.bytes_per_sample() * out.bytes_per_sample());
    for x in d.pixel_type.samples_f64(&d.data) {
        let y = m.map(x);
        match out {
            PixelType::Uint8 => v.push(y.clamp(0.0, 255.0) as u8),
            PixelType::Int8 => v.push(y.clamp(-128.0, 127.0) as i8 as u8),
            PixelType::Uint16 => v.extend((y.clamp(0.0, 65535.0) as u16).to_le_bytes()),
            PixelType::Int16 => v.extend((y.clamp(-32768.0, 32767.0) as i16).to_le_bytes()),
            PixelType::Uint32 => v.extend((y.clamp(0.0, f64::from(u32::MAX)) as u32).to_le_bytes()),
            PixelType::Int32 => {
                v.extend((y.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32).to_le_bytes())
            }
            PixelType::Double => v.extend(y.to_le_bytes()),
            _ => v.extend((y as f32).to_le_bytes()),
        }
    }
    v
}

/// Expand segmented palette data (PS3.3 C.7.9.2): discrete (0), linear (1) and indirect (2)
/// segments of 16-bit words. Linear segments are rounded half to even, as pydicom does.
fn expand_segmented(
    data: &[u16],
    max_segments: Option<usize>,
    last: Option<u16>,
    lut: &mut Vec<u16>,
    depth: u32,
) -> Result<()> {
    let bad = |m: &str| Error::corrupt(FORMAT_ID, format!("segmented palette: {m}"));
    if depth > 8 {
        return Err(bad("indirect segments nest too deep"));
    }
    let mut i = 0usize;
    let mut segments = 0usize;
    while i + 1 < data.len() {
        let (op, len) = (data[i], usize::from(data[i + 1]));
        i += 2;
        match op {
            0 => {
                let seg = data
                    .get(i..i + len)
                    .ok_or_else(|| bad("discrete segment runs past the data"))?;
                lut.extend_from_slice(seg);
                i += len;
            }
            1 => {
                let y0 = lut
                    .last()
                    .copied()
                    .or(last)
                    .ok_or_else(|| bad("starts with a linear segment"))?;
                let y1 = *data
                    .get(i)
                    .ok_or_else(|| bad("linear segment without its end value"))?;
                i += 1;
                if len > 0 {
                    let (a, b) = (f64::from(y0), f64::from(y1));
                    let step = (b - a) / len as f64;
                    let start = a + step;
                    for k in 0..len {
                        let v = if len == 1 {
                            b
                        } else {
                            start + (b - start) * k as f64 / (len - 1) as f64
                        };
                        lut.push(v.round_ties_even().clamp(0.0, 65535.0) as u16);
                    }
                }
            }
            2 => {
                let lo = u32::from(
                    *data
                        .get(i)
                        .ok_or_else(|| bad("indirect segment truncated"))?,
                );
                let hi = u32::from(
                    *data
                        .get(i + 1)
                        .ok_or_else(|| bad("indirect segment truncated"))?,
                );
                i += 2;
                let off = ((hi << 16) | lo) as usize;
                let rest = data
                    .get(off..)
                    .ok_or_else(|| bad("indirect offset outside the data"))?;
                let prev = lut.last().copied();
                expand_segmented(rest, Some(len), prev, lut, depth + 1)?;
            }
            other => return Err(bad(&format!("unknown segment type {other}"))),
        }
        if lut.len() > 1 << 16 {
            return Err(bad("expands past 65536 entries"));
        }
        segments += 1;
        if max_segments == Some(segments) {
            break;
        }
    }
    Ok(())
}

/// A palette colour lookup table (PS3.3 C.7.6.3.1.5-6).
#[derive(Debug, Clone)]
pub(crate) struct Palette {
    first: i64,
    rgb: [Vec<u16>; 3],
    /// 8-bit entries (output uint8), else uint16.
    pub(crate) eight_bit: bool,
}

impl Palette {
    /// The palette of a PALETTE COLOR image.
    pub(crate) fn of(file: &DicomFile, ds: &Item) -> Result<Palette> {
        let mut rgb: [Vec<u16>; 3] = Default::default();
        let mut first = 0i64;
        let mut bits = 16;
        for (i, (dt, dd)) in [
            (t::RED_PALETTE_DESCRIPTOR, t::RED_PALETTE_DATA),
            (t::GREEN_PALETTE_DESCRIPTOR, t::GREEN_PALETTE_DATA),
            (t::BLUE_PALETTE_DESCRIPTOR, t::BLUE_PALETTE_DATA),
        ]
        .into_iter()
        .enumerate()
        {
            let d = ds.numbers(dt);
            let segmented = ds.get(dd + 0x20);
            let e = ds.get(dd).or(segmented);
            let (Some(e), true) = (e, d.len() >= 3) else {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    "PALETTE COLOR image without palette descriptors and data",
                ));
            };
            let entries = if d[0] == 0.0 { 65536 } else { d[0] as usize };
            first = d[1] as i64;
            bits = d[2] as u32;
            let data = file.read_value(e)?;
            rgb[i] = if ds.get(dd).is_none() {
                if bits != 16 {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        "segmented palettes with 8-bit entries",
                        "Only 16-bit segmented palettes are expanded; `openreadout info` still reads the header.",
                    ));
                }
                let w = words(&data, data.len() / 2, e.little);
                let mut v = Vec::new();
                expand_segmented(&w, None, None, &mut v, 0)?;
                v.truncate(entries);
                v
            } else {
                words(&data, entries, e.little)
            };
            if rgb[i].is_empty() {
                return Err(Error::corrupt(FORMAT_ID, "empty palette data"));
            }
        }
        Ok(Palette {
            first,
            rgb,
            eight_bit: bits <= 8,
        })
    }

    /// Indices of a decoded frame to interleaved R, G, B.
    pub(crate) fn apply(&self, d: &Decoded) -> Vec<u8> {
        let mut out = Vec::new();
        for x in d.pixel_type.samples_f64(&d.data) {
            for c in &self.rgb {
                let i = (x as i64 - self.first).clamp(0, c.len() as i64 - 1) as usize;
                let v = c[i];
                if self.eight_bit {
                    // 8-bit entries in 16-bit words keep the value in the low byte; some
                    // writers put it in the high byte (both bytes equal is the common case)
                    out.push(if v > 255 { (v >> 8) as u8 } else { v as u8 });
                } else {
                    out.extend(v.to_le_bytes());
                }
            }
        }
        out
    }
}
