//! Velox EDS data: detector spectra (`Data/Spectrum/<id>`) and the X-ray event streams
//! (`Data/SpectrumStream/<id>`) from which spectrum images are assembled. Layout (our
//! derivation, `docs/formats/emd.md`, *Spectra and spectrum images*): a stream is uint16
//! values, 65535 ends a pixel and every other value is the energy channel of one detected
//! X-ray; pixels follow in raster order (x fastest), frame after frame.

use openreadout_core::{Error, Result};
use serde_json::Value;

use super::FORMAT_ID;

/// The value that ends a pixel in an event stream.
pub const PIXEL_END: u16 = 0xFFFF;
/// Most X-ray events decoded into memory for one spectrum image (4 bytes each).
pub const MAX_EVENTS: u64 = 400_000_000;

/// One `Data/Spectrum/<id>` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct VeloxSpectrum {
    /// The group name (a 32-hex-digit id).
    pub id: String,
    /// `BinaryResult.Detector` (`SuperXG21` … or the sum `SuperXG2`).
    pub detector: Option<String>,
    /// Number of energy channels.
    pub bins: u64,
    /// Energy of channel 0 and the width of a channel, eV (the detector's `OffsetEnergy` and
    /// `Dispersion`; for a sum spectrum, the value its segments share).
    pub offset_ev: Option<f64>,
    pub dispersion_ev: Option<f64>,
    /// The metadata document.
    pub metadata: Option<Value>,
}

impl VeloxSpectrum {
    /// Energy of channel `k` in keV, when calibrated.
    pub fn energy_kev(&self, k: u64) -> Option<f64> {
        Some((self.offset_ev? + k as f64 * self.dispersion_ev?) / 1000.0)
    }
}

/// One `Data/SpectrumStream/<id>` entry (header only).
#[derive(Debug, Clone, PartialEq)]
pub struct VeloxStream {
    /// The group name.
    pub id: String,
    /// Energy channels (`AcquisitionSettings.bincount`).
    pub bins: u32,
    /// Raster width and height (`RasterScanDefinition`); absent for point spectra.
    pub raster: Option<(u32, u32)>,
    /// Frames (`FrameLocationTable` rows; 1 when absent).
    pub frames: u64,
    /// uint16 values in the stream.
    pub values: u64,
}

/// X-ray events of a spectrum image, grouped by energy channel: the pixels of channel `c` are
/// `pixels[offsets[c]..offsets[c + 1]]` (one entry per event, pixel = `y * width + x`).
#[derive(Debug, Clone, Default)]
pub struct EventTable {
    pub offsets: Vec<u64>,
    pub pixels: Vec<u32>,
    /// Events per stream, in stream order.
    pub per_stream: Vec<u64>,
}

/// Decode one stream into `(channel, pixel)` events appended to `events`; `pixels` = raster
/// width × height. The stream must end every pixel of every frame: `frames × pixels` markers.
pub fn decode_stream(
    values: &[u8],
    bins: u32,
    pixels: u64,
    frames: u64,
    events: &mut Vec<(u16, u32)>,
) -> Result<u64> {
    let mut marker = 0u64;
    let mut n = 0u64;
    let want = pixels.saturating_mul(frames.max(1));
    for c in values.as_chunks::<2>().0 {
        let v = u16::from_le_bytes([c[0], c[1]]);
        if v == PIXEL_END {
            marker += 1;
            continue;
        }
        if u32::from(v) >= bins {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("event stream holds channel {v}, beyond its {bins} channels"),
            ));
        }
        if marker >= want {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("event stream has events after its last pixel ({want} pixel markers)"),
            ));
        }
        if events.len() as u64 >= MAX_EVENTS {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a spectrum image with more than {MAX_EVENTS} X-ray events"),
                "Read the detector spectra (`trace`) or the Velox images instead; very long acquisitions are not assembled in memory.",
            ));
        }
        let px = u32::try_from(marker % pixels.max(1)).unwrap_or(u32::MAX);
        events.push((v, px));
        n += 1;
    }
    if marker != want {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "event stream ends {marker} pixels, {want} expected ({frames} frame(s) of {pixels} pixels)"
            ),
        ));
    }
    Ok(n)
}

/// Group events by channel.
pub fn event_table(events: &[(u16, u32)], bins: u32, per_stream: Vec<u64>) -> EventTable {
    let mut offsets = vec![0u64; bins as usize + 1];
    for (c, _) in events {
        offsets[*c as usize + 1] += 1;
    }
    for i in 1..offsets.len() {
        offsets[i] += offsets[i - 1];
    }
    let mut next = offsets.clone();
    let mut pixels = vec![0u32; events.len()];
    for (c, p) in events {
        let slot = &mut next[*c as usize];
        pixels[*slot as usize] = *p;
        *slot += 1;
    }
    EventTable {
        offsets,
        pixels,
        per_stream,
    }
}

/// Histogram of a stream's events (for comparing with the stored detector spectra).
pub fn stream_histogram(values: &[u8], bins: u32) -> Vec<u64> {
    let mut h = vec![0u64; bins as usize];
    for c in values.as_chunks::<2>().0 {
        let v = u16::from_le_bytes([c[0], c[1]]);
        if v != PIXEL_END
            && let Some(slot) = h.get_mut(v as usize)
        {
            *slot += 1;
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn events_belong_to_the_pixel_their_marker_ends() {
        // 2 pixels, 2 frames: frame 0 pixel 0 has channel 3, pixel 1 has 1 and 2; frame 1
        // pixel 1 has channel 3.
        let s = stream(&[3, PIXEL_END, 1, 2, PIXEL_END, PIXEL_END, 3, PIXEL_END]);
        let mut ev = Vec::new();
        assert_eq!(decode_stream(&s, 4, 2, 2, &mut ev).unwrap(), 4);
        assert_eq!(ev, [(3, 0), (1, 1), (2, 1), (3, 1)]);
        let t = event_table(&ev, 4, vec![4]);
        assert_eq!(t.offsets, [0, 0, 1, 2, 4]);
        assert_eq!(&t.pixels[2..4], &[0, 1]);
        assert_eq!(stream_histogram(&s, 4), [0, 1, 1, 2]);
        // wrong marker count, channel out of range
        assert!(decode_stream(&s, 4, 2, 3, &mut Vec::new()).is_err());
        assert!(decode_stream(&stream(&[9, PIXEL_END]), 4, 1, 1, &mut Vec::new()).is_err());
    }
}
