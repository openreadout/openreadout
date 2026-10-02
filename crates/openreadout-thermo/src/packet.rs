//! Scan data packets: header, profile chunks, centroid list, per-peak descriptors, and the
//! frequency-to-m/z conversion. See `docs/formats/thermo-raw.md` § "Scan data packet".

use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::bytes::Cursor;

/// Bytes of the packet header.
pub const PACKET_HEADER_LEN: usize = 40;
/// Most acquisition segments (mass ranges) accepted in one packet.
pub const MAX_SEGMENTS: u32 = 64;
/// Zero-intensity bins the reference conversion emits on each side of a stored profile chunk.
pub const PROFILE_PAD_BINS: u32 = 4;
/// m/z step that keeps rendered profiles strictly increasing (see [`render_profile`]).
pub const MONOTONIC_NUDGE: f64 = 1e-5;
/// Descriptor flag bits (0x10 and 0x20) of peaks that the reference conversions leave out of
/// centroid lists and blank (zero intensity) in profiles.
pub const FLAG_EXCLUDED: u8 = 0x30;

/// The 40-byte header in front of every scan's data. Sizes are in 4-byte words; the six sized
/// parts follow the header in field order.
#[derive(Debug, Clone, Copy, Default)]
pub struct PacketHeader {
    /// Number of acquisition segments (mass ranges): 1 for ordinary scans, one per window in
    /// multi-range SIM scans; each segment after the first adds its range (two f32) after the
    /// header, and the centroid list is stored per segment.
    pub header_value: u32,
    pub profile_words: u32,
    pub centroid_words: u32,
    /// Bit 7 set: every profile chunk carries an m/z correction.
    pub layout_flags: u32,
    pub descriptor_words: u32,
    pub extra_words: u32,
    pub triplet_words: u32,
    /// Words of the peak-annotation block that closes the packet: 0 in files before the Orbitrap
    /// Exploris generation. Its layout is described in `docs/formats/thermo-raw.md`; it is
    /// skipped, as the reference conversions ignore it.
    pub annotation_words: u32,
    pub low_mz: f32,
    pub high_mz: f32,
}

impl PacketHeader {
    /// Total packet length the header implies, bytes.
    pub fn packet_len(&self) -> u64 {
        PACKET_HEADER_LEN as u64
            + 4 * (u64::from(self.extra_range_words())
                + u64::from(self.profile_words)
                + u64::from(self.centroid_words)
                + u64::from(self.descriptor_words)
                + u64::from(self.extra_words)
                + u64::from(self.triplet_words)
                + u64::from(self.annotation_words))
    }
    /// Acquisition segments the packet holds (`header_value` when it is 2 to [`MAX_SEGMENTS`],
    /// else 1).
    pub fn segments(&self) -> u32 {
        if (2..=MAX_SEGMENTS).contains(&self.header_value) {
            self.header_value
        } else {
            1
        }
    }
    /// Words of the segment ranges after the first, stored right after the header.
    pub fn extra_range_words(&self) -> u32 {
        2 * (self.segments() - 1)
    }
    pub fn chunks_have_correction(&self) -> bool {
        self.layout_flags & 0x80 != 0
    }
}

pub fn parse_packet_header(c: &mut Cursor<'_>) -> Result<PacketHeader> {
    Ok(PacketHeader {
        header_value: c.u32()?,
        profile_words: c.u32()?,
        centroid_words: c.u32()?,
        layout_flags: c.u32()?,
        descriptor_words: c.u32()?,
        extra_words: c.u32()?,
        triplet_words: c.u32()?,
        annotation_words: c.u32()?,
        low_mz: c.f32()?,
        high_mz: c.f32()?,
    })
}

/// A run of consecutive stored profile bins.
#[derive(Debug, Clone, Default)]
pub struct ProfileChunk {
    pub first_bin: u32,
    /// Additive m/z correction for every bin of this chunk (0 when the packet has none).
    pub mz_correction: f32,
    pub values: Vec<f32>,
}

/// Profile part of a packet: a uniform grid (frequency for Fourier-transform scans, m/z
/// otherwise) and the chunks of it that were stored.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub first_value: f64,
    pub step: f64,
    pub bin_count: u32,
    pub chunks: Vec<ProfileChunk>,
}

/// A stored centroid.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Centroid {
    pub mz: f64,
    pub intensity: f32,
}

/// Per-centroid descriptor word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeakDescriptor {
    pub peak_index: u16,
    pub flags: u8,
    pub descriptor_byte: u8,
}

/// A decoded packet.
#[derive(Debug, Clone, Default)]
pub struct Packet {
    pub header: PacketHeader,
    pub profile: Option<Profile>,
    pub centroids: Vec<Centroid>,
    pub descriptors: Vec<PeakDescriptor>,
    /// m/z range of every acquisition segment (one for ordinary scans).
    pub segment_ranges: Vec<[f32; 2]>,
    /// Set for scans stored as a [`WindowRecord`] (the header is then all zero).
    pub window_record: Option<WindowRecord>,
    /// One byte per centroid of a window-record scan (observed 0, 4, 6; meaning unknown).
    pub peak_flags: Vec<u8>,
}

/// Scan-index `kind_code` of scans stored as peaks followed by a [`WindowRecord`] instead of
/// a [`PacketHeader`] packet (every scan of the TSQ Vantage SRM file). For these the index
/// `data_size` is the window count plus one, not a byte count.
pub const WINDOW_RECORD_KIND: u32 = 24;
/// Bytes of one [`AcquisitionWindow`].
pub const WINDOW_LEN: u64 = 28;
/// Bytes a window-record peak takes: f32 m/z and f32 intensity, plus one flag byte stored after
/// the whole list.
pub const WINDOW_PEAK_LEN: u64 = 9;
/// Most windows we accept in one window record.
pub const MAX_WINDOWS: u32 = 4096;

/// One acquired m/z window of a window-record scan (an SRM product window).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AcquisitionWindow {
    pub low_mz: f32,
    pub high_mz: f32,
    /// Observed 2.0e-4 for every window (meaning unknown).
    pub window_value: f64,
    /// Offset of the scan's peak list from the scan-data address (first window only; 0 in the
    /// others).
    pub peak_offset: u32,
    /// Observed zero.
    pub window_words: [u32; 2],
}

/// The record the scan index points at for [`WINDOW_RECORD_KIND`] scans: u32 `lead_word`,
/// the windows, 16 reserved bytes, u32 `peak_bytes`, u32 `trailing_word`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowRecord {
    pub lead_word: u32,
    pub windows: Vec<AcquisitionWindow>,
    pub reserved: [u8; 16],
    /// Bytes of (m/z, intensity) pairs in the peak list (8 per peak).
    pub peak_bytes: u32,
    pub trailing_word: u32,
}

/// Length of a window record holding `windows` windows.
pub fn window_record_len(windows: u32) -> u64 {
    4 + WINDOW_LEN * u64::from(windows) + 24
}

pub fn parse_window_record(buf: &[u8], base: u64, windows: u32) -> Result<WindowRecord> {
    let mut c = Cursor::new(buf, base);
    let lead_word = c.u32()?;
    let mut out = Vec::with_capacity((windows as usize).min(c.remaining() / WINDOW_LEN as usize));
    for _ in 0..windows {
        out.push(AcquisitionWindow {
            low_mz: c.f32()?,
            high_mz: c.f32()?,
            window_value: c.f64()?,
            peak_offset: c.u32()?,
            window_words: [c.u32()?, c.u32()?],
        });
    }
    let mut reserved = [0u8; 16];
    reserved.copy_from_slice(c.take(16)?);
    let at = c.offset();
    let peak_bytes = c.u32()?;
    if peak_bytes % 8 != 0 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("window record lists {peak_bytes} peak bytes, not a multiple of 8"),
        ));
    }
    Ok(WindowRecord {
        lead_word,
        windows: out,
        reserved,
        peak_bytes,
        trailing_word: c.u32()?,
    })
}

/// Decode the peak list of a window-record scan: `n` (f32 m/z, f32 intensity) pairs, then `n`
/// flag bytes.
pub fn parse_window_peaks(buf: &[u8], base: u64, n: usize) -> Result<(Vec<Centroid>, Vec<u8>)> {
    let mut c = Cursor::new(buf, base);
    let mut peaks = Vec::with_capacity(n.min(c.remaining() / 8));
    for _ in 0..n {
        peaks.push(Centroid {
            mz: f64::from(c.f32()?),
            intensity: c.f32()?,
        });
    }
    let flags = c.take(n)?.to_vec();
    Ok((peaks, flags))
}

/// Decode a whole packet from `buf` (which must start at the packet header).
pub fn parse_packet(buf: &[u8], base: u64) -> Result<Packet> {
    let mut c = Cursor::new(buf, base);
    let header = parse_packet_header(&mut c)?;
    let mut segment_ranges = vec![[header.low_mz, header.high_mz]];
    for _ in 1..header.segments() {
        segment_ranges.push([c.f32()?, c.f32()?]);
    }
    if header.segments() > 1 && header.profile_words > 0 {
        return Err(Error::unsupported(
            FORMAT_ID,
            "profile data of multi-range (SIM) scans",
            "Only centroided multi-range scans have been seen; read the scan's centroids (`--centroid`) or convert with the vendor's software.",
        ));
    }
    let body_start = c.position();
    let profile_end = body_start
        .checked_add(header.profile_words as usize * 4)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, base, "profile size overflows"))?;
    let profile = if header.profile_words > 0 {
        let p = parse_profile(&mut c, header.chunks_have_correction())?;
        if c.position() != profile_end {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                c.offset(),
                format!(
                    "profile chunks end at word {} but the header says {}",
                    (c.position() - body_start) / 4,
                    header.profile_words
                ),
            ));
        }
        Some(p)
    } else {
        None
    };
    c.seek_to(profile_end)?;
    let centroids = if header.centroid_words > 0 {
        parse_centroids(&mut c, header.centroid_words, header.segments())?
    } else {
        Vec::new()
    };
    let mut descriptors =
        Vec::with_capacity((header.descriptor_words as usize).min(c.remaining() / 4));
    for _ in 0..header.descriptor_words {
        let w = c.take(4)?;
        descriptors.push(PeakDescriptor {
            peak_index: u16::from_le_bytes([w[0], w[1]]),
            flags: w[2],
            descriptor_byte: w[3],
        });
    }
    Ok(Packet {
        header,
        profile,
        centroids,
        descriptors,
        segment_ranges,
        window_record: None,
        peak_flags: Vec::new(),
    })
}

fn parse_profile(c: &mut Cursor<'_>, with_correction: bool) -> Result<Profile> {
    let first_value = c.f64()?;
    let step = c.f64()?;
    let nchunks = c.u32()?;
    let bin_count = c.u32()?;
    let mut chunks = Vec::with_capacity((nchunks as usize).min(c.remaining() / 8));
    for _ in 0..nchunks {
        let at = c.offset();
        let first_bin = c.u32()?;
        let n = c.u32()?;
        let mz_correction = if with_correction { c.f32()? } else { 0.0 };
        if n as usize > c.remaining() / 4 {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("profile chunk of {n} bins runs past the packet"),
            ));
        }
        let mut values = Vec::with_capacity(n as usize);
        for _ in 0..n {
            values.push(c.f32()?);
        }
        chunks.push(ProfileChunk {
            first_bin,
            mz_correction,
            values,
        });
    }
    Ok(Profile {
        first_value,
        step,
        bin_count,
        chunks,
    })
}

/// Centroid list: per acquisition segment a u32 count, then 8-byte (f32 m/z, f32 intensity)
/// or 12-byte (f64 m/z, f32 intensity) records; the word count of the list says which.
fn parse_centroids(c: &mut Cursor<'_>, words: u32, segments: u32) -> Result<Vec<Centroid>> {
    let at = c.offset();
    let start = c.position();
    // Which record width makes the segments' counts fill the list exactly.
    let fits = |c: &mut Cursor<'_>, per: u64| -> Result<bool> {
        c.seek_to(start)?;
        let mut used = 0u64;
        for _ in 0..segments {
            if used >= u64::from(words) {
                return Ok(false);
            }
            let n = u64::from(c.u32()?);
            used += 1 + per * n;
            if used > u64::from(words) {
                return Ok(false);
            }
            c.seek_to(start + 4 * used as usize)?;
        }
        Ok(used == u64::from(words))
    };
    let wide = if fits(c, 2).unwrap_or(false) {
        false
    } else if fits(c, 3).unwrap_or(false) {
        true
    } else {
        c.seek_to(start)?;
        let n = c.u32()?;
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!(
                "centroid list of {n} peaks ({segments} segment(s)) does not fit its {words} words"
            ),
        ));
    };
    c.seek_to(start)?;
    let mut out = Vec::new();
    for _ in 0..segments {
        let n = c.u32()?;
        out.reserve((n as usize).min(c.remaining() / 8));
        for _ in 0..n {
            let mz = if wide { c.f64()? } else { f64::from(c.f32()?) };
            out.push(Centroid {
                mz,
                intensity: c.f32()?,
            });
        }
    }
    Ok(out)
}

/// How a profile grid value maps to m/z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MzScale {
    /// The grid is already m/z.
    Direct,
    /// Orbitrap form: `A + B/f² + C/f⁴`.
    InverseSquare { a: f64, b: f64, c: f64 },
    /// Ion-cyclotron form: `A + B/f + C/f²`.
    Inverse { a: f64, b: f64, c: f64 },
}

impl MzScale {
    /// Choose the conversion from the grid step sign and the scan event's coefficients.
    pub fn from_event(step: f64, coefficients: &[f64]) -> Option<Self> {
        if step > 0.0 {
            return Some(MzScale::Direct);
        }
        match coefficients.len() {
            5 | 7 => Some(MzScale::InverseSquare {
                a: coefficients[2],
                b: coefficients[3],
                c: coefficients[4],
            }),
            4 => Some(MzScale::Inverse {
                a: coefficients[1],
                b: coefficients[2],
                c: coefficients[3],
            }),
            _ => None,
        }
    }

    /// m/z of a grid value. The evaluation order matters for bit-exact agreement.
    pub fn mz(self, f: f64) -> f64 {
        match self {
            MzScale::Direct => f,
            MzScale::InverseSquare { a, b, c } => {
                let f2 = f * f;
                a + b / f2 + c / (f2 * f2)
            }
            MzScale::Inverse { a, b, c } => a + b / f + c / (f * f),
        }
    }
}

/// Render a profile the way the reference conversion does (validated bit-for-bit on the corpus):
/// every stored chunk plus [`PROFILE_PAD_BINS`] zero bins on each side, grid bins `1..=4` and
/// `n-3..=n` as zeros, padding bins taking the correction of the next chunk, and chunks whose
/// m/z span contains a centroid flagged [`FLAG_EXCLUDED`] reported with zero intensity (unless
/// `include_flagged`).
pub fn render_profile(
    profile: &Profile,
    scale: MzScale,
    centroids: &[Centroid],
    descriptors: &[PeakDescriptor],
    include_flagged: bool,
) -> (Vec<f64>, Vec<f32>) {
    let chunks = &profile.chunks;
    if chunks.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let grid = |bin: u32, corr: f32| {
        scale.mz(profile.first_value + profile.step * f64::from(bin)) + f64::from(corr)
    };
    // Which chunks the converter blanks.
    let mut blank = vec![false; chunks.len()];
    if !include_flagged {
        for (i, d) in descriptors.iter().enumerate() {
            if d.flags & FLAG_EXCLUDED == 0 {
                continue;
            }
            let Some(peak) = centroids.get(i) else {
                continue;
            };
            for (k, ch) in chunks.iter().enumerate() {
                let n = ch.values.len() as u32;
                if n == 0 {
                    continue;
                }
                let lo = grid(ch.first_bin, ch.mz_correction);
                let hi = grid(ch.first_bin.saturating_add(n - 1), ch.mz_correction);
                let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                if lo <= peak.mz && peak.mz <= hi {
                    blank[k] = true;
                }
            }
        }
    }
    // Collect (bin, value, correction) with stored bins taking precedence over padding.
    // An m/z grid (ion-trap profiles, `MzScale::Direct`) numbers its stored values from bin 1:
    // value j of a chunk lies at bin first_bin + j + 1, and values past `bin_count` are not
    // part of the scan (every ion-trap profile of the LTQ Velos and SPS reference conversions:
    // the reference's m/z are one step above bin first_bin + j, and it lists one point fewer).
    let direct = matches!(scale, MzScale::Direct);
    let mut points: std::collections::BTreeMap<u32, (f32, f32, bool)> =
        std::collections::BTreeMap::new();
    for (k, ch) in chunks.iter().enumerate() {
        for (j, &v) in ch.values.iter().enumerate() {
            let bin = ch.first_bin.saturating_add(j as u32);
            let bin = if direct {
                let b = bin.saturating_add(1);
                if b > profile.bin_count {
                    continue;
                }
                b
            } else {
                bin
            };
            let v = if blank[k] { 0.0 } else { v };
            points.insert(bin, (v, ch.mz_correction, true));
        }
    }
    let last = chunks.len() - 1;
    // A padding bin takes the correction of the chunk that contains it, else of the first chunk
    // starting after it, else of the last chunk.
    let starts: Vec<u32> = chunks.iter().map(|c| c.first_bin).collect();
    let pad_correction = |bin: u32| {
        let k = starts.partition_point(|&s| s <= bin);
        if k > 0 {
            let ch = &chunks[k - 1];
            if u64::from(bin) < u64::from(ch.first_bin) + ch.values.len() as u64 {
                return ch.mz_correction;
            }
        }
        chunks[k.min(last)].mz_correction
    };
    // Padding stays on the grid's bins 1..=bin_count (bin 0 is never emitted as padding).
    let mut pad = |bin: u32| {
        if (1..=profile.bin_count).contains(&bin) {
            points
                .entry(bin)
                .or_insert_with(|| (0.0, pad_correction(bin), false));
        }
    };
    for ch in chunks {
        let first = ch.first_bin.saturating_add(u32::from(direct));
        let end = first.saturating_add(ch.values.len() as u32);
        for d in 1..=PROFILE_PAD_BINS {
            if let Some(b) = first.checked_sub(d) {
                pad(b);
            }
            if let Some(b) = end.checked_add(d - 1) {
                pad(b);
            }
        }
    }
    for b in 1..=PROFILE_PAD_BINS {
        pad(b);
        if let Some(e) = profile
            .bin_count
            .checked_add(1)
            .and_then(|n| n.checked_sub(b))
        {
            pad(e);
        }
    }
    let mut mz: Vec<f64> = Vec::with_capacity(points.len());
    let mut it = Vec::with_capacity(points.len());
    for (bin, (v, corr, _)) in points {
        let mut m = grid(bin, corr);
        // Neighbouring chunks can carry very different corrections; the reference conversion
        // keeps m/z strictly increasing by placing such a point 1e-5 above its predecessor.
        if let Some(&prev) = mz.last()
            && m <= prev
        {
            m = prev + MONOTONIC_NUDGE;
        }
        mz.push(m);
        it.push(v);
    }
    (mz, it)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scan 5 of mtbls1822 laid out as stored: one peak, its flag, then a one-window record.
    #[test]
    fn window_record_round_trip() {
        let mut peaks = Vec::new();
        peaks.extend_from_slice(&105.136_f32.to_le_bytes());
        peaks.extend_from_slice(&0.8147_f32.to_le_bytes());
        peaks.push(0);
        let mut rec = Vec::new();
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(&105.135_f32.to_le_bytes());
        rec.extend_from_slice(&105.137_f32.to_le_bytes());
        rec.extend_from_slice(&2.0e-4_f64.to_le_bytes());
        rec.extend_from_slice(&510u32.to_le_bytes());
        rec.extend_from_slice(&[0u8; 8]);
        rec.extend_from_slice(&[0u8; 16]);
        rec.extend_from_slice(&8u32.to_le_bytes());
        rec.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(rec.len() as u64, window_record_len(1));
        let r = parse_window_record(&rec, 519, 1).unwrap();
        assert_eq!(r.windows.len(), 1);
        assert_eq!(r.windows[0].peak_offset, 510);
        assert_eq!(r.peak_bytes, 8);
        let (p, flags) = parse_window_peaks(&peaks, 510, 1).unwrap();
        assert_eq!(p[0].mz.to_bits(), f64::from(105.136_f32).to_bits());
        assert_eq!(p[0].intensity.to_bits(), 0.8147_f32.to_bits());
        assert_eq!(flags, vec![0]);
        // A record cut short, or a peak byte count that is not whole peaks, is an error.
        assert!(parse_window_record(&rec[..rec.len() - 1], 519, 1).is_err());
        let mut bad = rec.clone();
        let at = bad.len() - 8;
        bad[at..at + 4].copy_from_slice(&7u32.to_le_bytes());
        assert!(parse_window_record(&bad, 519, 1).is_err());
        assert!(parse_window_peaks(&peaks[..8], 510, 1).is_err());
    }

    fn chunk(first_bin: u32, values: &[f32], corr: f32) -> ProfileChunk {
        ProfileChunk {
            first_bin,
            mz_correction: corr,
            values: values.to_vec(),
        }
    }

    #[test]
    fn padding_and_edges() {
        let p = Profile {
            first_value: 100.0,
            step: 1.0,
            bin_count: 40,
            chunks: vec![chunk(20, &[1.0, 2.0], 0.0)],
        };
        let (mz, it) = render_profile(&p, MzScale::Direct, &[], &[], false);
        let bins: Vec<i64> = mz.iter().map(|m| (*m - 100.0) as i64).collect();
        assert_eq!(
            bins,
            vec![
                // an m/z grid places the chunk's values one bin up (21, 22)
                1, 2, 3, 4, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 37, 38, 39, 40
            ]
        );
        assert_eq!(it.iter().filter(|v| **v != 0.0).count(), 2);
    }

    #[test]
    fn blanked_chunk_is_zeroed() {
        let p = Profile {
            first_value: 100.0,
            step: 1.0,
            bin_count: 100,
            chunks: vec![chunk(20, &[5.0, 6.0, 5.0], 0.0)],
        };
        let c = [Centroid {
            mz: 121.0,
            intensity: 6.0,
        }];
        let d = [PeakDescriptor {
            peak_index: 0,
            flags: 0x90,
            descriptor_byte: 0,
        }];
        let (_, it) = render_profile(&p, MzScale::Direct, &c, &d, false);
        assert!(it.iter().all(|v| *v == 0.0));
        let (_, it) = render_profile(&p, MzScale::Direct, &c, &d, true);
        assert_eq!(it.iter().filter(|v| **v != 0.0).count(), 3);
    }

    #[test]
    fn annotation_words_count_in_the_packet_length() {
        // One f32 centroid (3 words), its descriptor (1 word) and a 3-word annotation block.
        let mut buf = Vec::new();
        for w in [1u32, 0, 3, 0, 1, 0, 0, 3] {
            buf.extend_from_slice(&w.to_le_bytes());
        }
        buf.extend_from_slice(&100.0f32.to_le_bytes());
        buf.extend_from_slice(&200.0f32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&150.0f32.to_le_bytes());
        buf.extend_from_slice(&42.0f32.to_le_bytes());
        buf.extend_from_slice(&[0, 0, 0x10, 0]);
        for w in [0x000A_000Fu32, 0x0008_0000, 0] {
            buf.extend_from_slice(&w.to_le_bytes());
        }
        let p = parse_packet(&buf, 0).unwrap();
        assert_eq!(p.header.annotation_words, 3);
        assert_eq!(p.header.packet_len(), buf.len() as u64);
        assert_eq!(p.centroids.len(), 1);
        assert_eq!(p.descriptors[0].flags, 0x10);
    }

    #[test]
    fn centroid_width_from_word_count() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&500.25f64.to_le_bytes());
        buf.extend_from_slice(&7.0f32.to_le_bytes());
        let mut c = Cursor::new(&buf, 0);
        let v = parse_centroids(&mut c, 4, 1).unwrap();
        assert!((v[0].mz - 500.25).abs() < f64::EPSILON);
        let mut c = Cursor::new(&buf, 0);
        assert!(parse_centroids(&mut c, 5, 1).is_err());
        // two segments of one and two narrow peaks
        let mut two = Vec::new();
        for (count, peaks) in [
            (1u32, vec![(100.5f32, 1.0f32)]),
            (2, vec![(200.25, 2.0), (201.0, 3.0)]),
        ] {
            two.extend_from_slice(&count.to_le_bytes());
            for (m, i) in peaks {
                two.extend_from_slice(&m.to_le_bytes());
                two.extend_from_slice(&i.to_le_bytes());
            }
        }
        let mut c = Cursor::new(&two, 0);
        let v = parse_centroids(&mut c, 8, 2).unwrap();
        assert_eq!(v.len(), 3);
        assert!((v[2].mz - 201.0).abs() < f64::EPSILON);
    }
}
