//! Frame blobs of `analysis.tdf_bin` (TDF) and `analysis.tsf_bin` (TSF).
//!
//! Layouts as documented by timsrust (Apache-2.0); see `docs/formats/bruker-tdf.md` and
//! `docs/provenance/bruker-tdf.md`.

use std::io::{Read, Seek, SeekFrom};

use openreadout_core::bytes::le_u32;
use openreadout_core::source::SourceFile;

/// One decoded TDF frame: a sparse (scan × TOF index) intensity matrix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimsFrame {
    /// `scan_offsets[s]..scan_offsets[s + 1]` are the peaks of scan `s` (length `scans + 1`).
    pub scan_offsets: Vec<usize>,
    pub tof_indices: Vec<u32>,
    pub intensities: Vec<u32>,
}

impl TimsFrame {
    pub fn scan_count(&self) -> usize {
        self.scan_offsets.len().saturating_sub(1)
    }
    /// Peaks of scans `first..end` (end exclusive, clamped to the frame).
    pub fn scan_range(&self, first: usize, end: usize) -> (&[u32], &[u32]) {
        let n = self.scan_count();
        let (a, b) = (first.min(n), end.min(n).max(first.min(n)));
        let (lo, hi) = (self.scan_offsets[a], self.scan_offsets[b]);
        (&self.tof_indices[lo..hi], &self.intensities[lo..hi])
    }
}

/// Why a frame blob could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameError(pub String);

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, FrameError> {
    Err(FrameError(m.into()))
}

/// Largest blob we accept (a frame is a few MB at most; this bounds hostile headers).
const MAX_BLOB: u64 = 1 << 30;
/// Most mobility scans one frame may declare (instruments write about a thousand).
const MAX_SCANS: u32 = 1 << 20;

/// Read the raw blob at `offset`: the `u32` byte count (header included), the second header
/// `u32`, and the bytes after the 8-byte header.
pub fn read_blob(
    bin: &mut SourceFile,
    bin_len: u64,
    offset: u64,
) -> Result<(u32, Vec<u8>), FrameError> {
    if offset.checked_add(8).is_none_or(|e| e > bin_len) {
        return err(format!(
            "blob offset {offset} is past the end of the binary file ({bin_len} bytes)"
        ));
    }
    let mut hdr = [0u8; 8];
    bin.seek(SeekFrom::Start(offset))
        .and_then(|_| bin.read_exact(&mut hdr))
        .map_err(|e| FrameError(format!("read at {offset}: {e}")))?;
    let total = u64::from(le_u32(&hdr, 0).unwrap_or(0));
    let second = le_u32(&hdr, 4).unwrap_or(0);
    if !(8..=MAX_BLOB).contains(&total) {
        return err(format!("blob at {offset} declares {total} bytes"));
    }
    if offset + total > bin_len {
        return err(format!(
            "blob at {offset} needs {total} bytes but the binary file ends at {bin_len} (truncated)"
        ));
    }
    let mut data = vec![0u8; usize::try_from(total - 8).unwrap_or(0)];
    bin.read_exact(&mut data)
        .map_err(|e| FrameError(format!("read at {offset}: {e}")))?;
    Ok((second, data))
}

/// Decode a compression-type-2 TDF frame blob (zstd; four byte planes of `u32` values).
pub fn decode_tdf_frame(compressed: &[u8], scans_hint: u32) -> Result<TimsFrame, FrameError> {
    if compressed.is_empty() {
        // An empty blob is a frame without peaks; its scan count comes from the blob header
        // or the Frames table, both file data. Refuse counts no instrument writes instead of
        // allocating an offset table of up to 2^32 entries (32 GiB).
        if scans_hint > MAX_SCANS {
            return err(format!(
                "empty frame declares {scans_hint} scans (more than {MAX_SCANS})"
            ));
        }
        return Ok(TimsFrame {
            scan_offsets: vec![0; scans_hint as usize + 1],
            ..TimsFrame::default()
        });
    }
    let bytes = openreadout_codecs::zstd_decode(compressed, 0)
        .map_err(|e| FrameError(format!("zstd: {e}")))?;
    if !bytes.len().is_multiple_of(4) {
        return err(format!(
            "decompressed frame is {} bytes, not a multiple of 4",
            bytes.len()
        ));
    }
    let n = bytes.len() / 4;
    let get = |i: usize| -> Option<u32> {
        if i >= n {
            return None;
        }
        Some(u32::from_le_bytes([
            bytes[i],
            bytes[n + i],
            bytes[2 * n + i],
            bytes[3 * n + i],
        ]))
    };
    let scans = get(0).ok_or_else(|| FrameError("empty frame".into()))? as usize;
    if scans == 0 || scans > n {
        return err(format!("frame declares {scans} scans in {n} values"));
    }
    let peaks = (n - scans) / 2;
    let mut offsets = Vec::with_capacity(scans + 1);
    offsets.push(0usize);
    for s in 0..scans - 1 {
        let doubled = get(s + 1).ok_or_else(|| FrameError("scan table cut short".into()))? as usize;
        let next = offsets[s] + doubled / 2;
        if next > peaks {
            return err(format!("scan {s} runs past the frame's {peaks} peaks"));
        }
        offsets.push(next);
    }
    offsets.push(peaks);
    let mut intensities = Vec::with_capacity(peaks);
    let mut tofs = Vec::with_capacity(peaks);
    for p in 0..peaks {
        intensities
            .push(get(scans + 1 + 2 * p).ok_or_else(|| FrameError("intensity cut short".into()))?);
    }
    for s in 0..scans {
        let mut sum: u32 = 0;
        for p in offsets[s]..offsets[s + 1] {
            let delta =
                get(scans + 2 * p).ok_or_else(|| FrameError("TOF delta cut short".into()))?;
            sum = sum
                .checked_add(delta)
                .ok_or_else(|| FrameError(format!("TOF index overflows in scan {s}")))?;
            tofs.push(
                sum.checked_sub(1)
                    .ok_or_else(|| FrameError(format!("TOF index below zero in scan {s}")))?,
            );
        }
    }
    Ok(TimsFrame {
        scan_offsets: offsets,
        tof_indices: tofs,
        intensities,
    })
}

/// Decode a compression-type-1 TDF frame blob (first-generation timsTOF, LZF). `data` is the
/// blob after its 8-byte header and `scans` the scan count from that header. It starts with
/// `scans + 1` `u32` offsets, counted from the start of the blob (header included), of each
/// scan's LZF stream; a scan whose offsets are equal is empty. A decompressed scan is a run of
/// little-endian `i32` values: a positive value is the intensity at the current TOF index,
/// which then advances by one; zero or a negative value `-k` skips `k` TOF indices.
/// `max_scan_bytes` bounds one scan's decompressed size.
pub fn decode_tdf_frame_lzf(
    data: &[u8],
    scans: u32,
    max_scan_bytes: usize,
) -> Result<TimsFrame, FrameError> {
    if scans > MAX_SCANS {
        return err(format!(
            "frame declares {scans} scans (more than {MAX_SCANS})"
        ));
    }
    let n = scans as usize;
    let table = (n + 1) * 4;
    if data.len() < table {
        return err(format!(
            "blob of {} bytes cannot hold the offsets of {n} scans",
            data.len()
        ));
    }
    let off = |i: usize| -> usize { le_u32(data, i * 4).unwrap_or(0) as usize };
    let mut scan_offsets = Vec::with_capacity(n + 1);
    scan_offsets.push(0usize);
    let mut tofs: Vec<u32> = Vec::new();
    let mut intensities: Vec<u32> = Vec::new();
    for s in 0..n {
        let (a, b) = (off(s), off(s + 1));
        // Offsets count the 8-byte header; the stream of scan `s` is data[a-8..b-8].
        let (a, b) = match (a.checked_sub(8), b.checked_sub(8)) {
            (Some(a), Some(b)) if a >= table && a <= b && b <= data.len() => (a, b),
            _ => {
                return err(format!(
                    "scan {s}: stream offsets {a}..{b} lie outside the {}-byte blob",
                    data.len() + 8
                ));
            }
        };
        if a < b {
            let raw = openreadout_codecs::lzf_decode(&data[a..b], max_scan_bytes)
                .map_err(|e| FrameError(format!("scan {s}: {e}")))?;
            if !raw.len().is_multiple_of(4) {
                return err(format!(
                    "scan {s}: {} decompressed bytes, not whole 32-bit values",
                    raw.len()
                ));
            }
            let mut next: u32 = 0;
            for c in raw.as_chunks::<4>().0 {
                let v = i32::from_le_bytes(*c);
                if v > 0 {
                    tofs.push(next);
                    intensities.push(v.unsigned_abs());
                    next = next
                        .checked_add(1)
                        .ok_or_else(|| FrameError(format!("scan {s}: TOF index overflows")))?;
                } else {
                    next = next
                        .checked_add(v.unsigned_abs())
                        .ok_or_else(|| FrameError(format!("scan {s}: TOF index overflows")))?;
                }
            }
        }
        scan_offsets.push(tofs.len());
    }
    Ok(TimsFrame {
        scan_offsets,
        tof_indices: tofs,
        intensities,
    })
}

/// Decode a TSF line-spectrum blob: `peaks` `f64` TOF indices, then `peaks` `f32` intensities.
pub fn decode_tsf_spectrum(
    compressed_len: u32,
    data: &[u8],
    peaks: usize,
) -> Result<(Vec<f64>, Vec<f32>), FrameError> {
    let clen = compressed_len as usize;
    if clen > data.len() {
        return err(format!(
            "compressed length {clen} exceeds the chunk ({} bytes)",
            data.len()
        ));
    }
    if clen == 0 || peaks == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let bytes = openreadout_codecs::zstd_decode(&data[..clen], 0)
        .map_err(|e| FrameError(format!("zstd: {e}")))?;
    let need = peaks
        .checked_mul(12)
        .ok_or_else(|| FrameError("peak count overflows".into()))?;
    if bytes.len() < need {
        return err(format!(
            "{} bytes decoded, {need} needed for {peaks} peaks",
            bytes.len()
        ));
    }
    let (t, i) = bytes.split_at(peaks * 8);
    let tofs = t
        .as_chunks::<8>()
        .0
        .iter()
        .map(|&c| f64::from_le_bytes(c))
        .collect();
    let ints = i[..peaks * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| f32::from_le_bytes(c))
        .collect();
    Ok((tofs, ints))
}

/// Decode a TSF profile spectrum: the zstd frame after the `line_len`-byte line-spectrum
/// frame of the blob (`data` is the blob after its 8-byte header), holding one `u32` per
/// digitizer sample in four byte planes (as TDF frames store their values). `samples` is
/// `DigitizerNumSamples`; the decoded count must equal it.
pub fn decode_tsf_profile(
    line_len: u32,
    data: &[u8],
    samples: usize,
) -> Result<Vec<u32>, FrameError> {
    let start = line_len as usize;
    if start >= data.len() {
        return err("the frame holds no profile spectrum after its line spectrum");
    }
    if samples == 0 || samples > (1 << 26) {
        return err(format!(
            "DigitizerNumSamples {samples} is not a usable profile length"
        ));
    }
    let bytes = openreadout_codecs::zstd_decode(&data[start..], samples * 4)
        .map_err(|e| FrameError(format!("profile zstd: {e}")))?;
    if bytes.len() != samples * 4 {
        return err(format!(
            "profile decodes to {} bytes, {} expected for {samples} samples",
            bytes.len(),
            samples * 4
        ));
    }
    let n = samples;
    Ok((0..n)
        .map(|i| u32::from_le_bytes([bytes[i], bytes[n + i], bytes[2 * n + i], bytes[3 * n + i]]))
        .collect())
}

/// Sum intensities per TOF index: sorted unique indices and their `u64` sums.
pub fn group_and_sum<'a>(
    parts: impl IntoIterator<Item = (&'a [u32], &'a [u32])>,
) -> (Vec<u32>, Vec<u64>) {
    let mut pairs: Vec<(u32, u64)> = Vec::new();
    for (t, i) in parts {
        pairs.extend(t.iter().zip(i).map(|(&a, &b)| (a, u64::from(b))));
    }
    pairs.sort_unstable_by_key(|p| p.0);
    let mut tof = Vec::with_capacity(pairs.len());
    let mut sum: Vec<u64> = Vec::with_capacity(pairs.len());
    for (t, v) in pairs {
        if tof.last() == Some(&t) {
            if let Some(s) = sum.last_mut() {
                *s += v;
            }
        } else {
            tof.push(t);
            sum.push(v);
        }
    }
    (tof, sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the planar u32 layout and compress it (test helper).
    fn blob(values: &[u32]) -> Vec<u8> {
        let n = values.len();
        let mut planes = vec![0u8; n * 4];
        for (i, v) in values.iter().enumerate() {
            let b = v.to_le_bytes();
            for k in 0..4 {
                planes[k * n + i] = b[k];
            }
        }
        ruzstd::encoding::compress_to_vec(
            planes.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    #[test]
    fn two_scans() {
        // 2 scans; scan 0 has 2 peaks (doubled count 4); peaks: (tof delta, intensity)
        // scan 0: deltas 11, 5 -> tof 10, 15; scan 1: delta 3 -> tof 2
        let v = [2, 4, 11, 100, 5, 200, 3, 300];
        let f = decode_tdf_frame(&blob(&v), 2).unwrap();
        assert_eq!(f.scan_offsets, vec![0, 2, 3]);
        assert_eq!(f.tof_indices, vec![10, 15, 2]);
        assert_eq!(f.intensities, vec![100, 200, 300]);
        let (t, i) = group_and_sum([f.scan_range(0, 2)]);
        assert_eq!(t, vec![2, 10, 15]);
        assert_eq!(i, vec![300, 100, 200]);
    }

    #[test]
    fn malformed() {
        assert!(decode_tdf_frame(&blob(&[0]), 1).is_err());
        assert!(decode_tdf_frame(&blob(&[2, 100, 1, 1]), 2).is_err());
        assert!(decode_tdf_frame(&[1, 2, 3], 1).is_err());
        assert!(decode_tdf_frame(&blob(&[1, 0, 5]), 1).is_err()); // tof 0 - 1
    }
}
