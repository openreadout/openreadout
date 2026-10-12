//! A large baseline JPEG read by restart intervals: each interval (one MCU row high, `DRI`
//! MCUs wide) decodes on its own as a small JPEG made of the file's header with the frame size
//! set to the interval's, the interval's entropy-coded bytes and an end marker.

use openreadout_core::bytes::be_u16;
use openreadout_core::{Error, Result};

use super::FORMAT_ID;

/// The parsed header of one image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegLayout {
    /// SOI up to and including SOS, without the `DRI` segment.
    pub header: Vec<u8>,
    /// Offset of the SOF segment's marker in `header`.
    pub sof_at: usize,
    /// Image width and height.
    pub width: u32,
    /// See `width`.
    pub height: u32,
    /// MCU width and height in pixels.
    pub mcu_w: u32,
    /// See `mcu_w`.
    pub mcu_h: u32,
    /// MCUs per restart interval (0: none).
    pub restart: u32,
    /// Byte offset of the first entropy-coded byte (right after SOS).
    pub scan_start: u64,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, format!("JPEG image: {}", detail.into()))
}

impl JpegLayout {
    /// Parse the header from the file's first bytes (`bytes` must reach past SOS).
    pub fn parse(bytes: &[u8]) -> Result<JpegLayout> {
        if bytes.get(..2) != Some(&[0xFF, 0xD8][..]) {
            return Err(bad("no SOI marker"));
        }
        let mut header = vec![0xFF, 0xD8];
        let mut i = 2usize;
        let (mut sof_at, mut restart) = (None, 0u32);
        let (mut w, mut h, mut mcu_w, mut mcu_h) = (0u32, 0u32, 0u32, 0u32);
        loop {
            let (Some(&ff), Some(&m)) = (bytes.get(i), bytes.get(i + 1)) else {
                return Err(bad("no start-of-scan marker in the header"));
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
                0xC0 | 0xC1 => {
                    let nf = usize::from(*seg.get(9).ok_or_else(|| bad("short SOF"))?);
                    if seg.get(4) != Some(&8) {
                        return Err(Error::unsupported(
                            FORMAT_ID,
                            "a JPEG image that is not 8-bit",
                            "Only 8-bit JPEG images are known in VMS slides.",
                        ));
                    }
                    h = u32::from(be_u16(seg, 5).unwrap_or(0));
                    w = u32::from(be_u16(seg, 7).unwrap_or(0));
                    let (mut hmax, mut vmax) = (1u32, 1u32);
                    for k in 0..nf {
                        let hv = *seg.get(11 + 3 * k).ok_or_else(|| bad("short SOF"))?;
                        hmax = hmax.max(u32::from(hv >> 4));
                        vmax = vmax.max(u32::from(hv & 15));
                    }
                    mcu_w = 8 * hmax;
                    mcu_h = 8 * vmax;
                    sof_at = Some(header.len());
                    header.extend_from_slice(seg);
                }
                0xC2..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        "a JPEG image that is not baseline or extended sequential",
                        "Only baseline JPEG images with restart intervals are read in VMS slides.",
                    ));
                }
                0xDD => {
                    restart = u32::from(be_u16(seg, 4).ok_or_else(|| bad("short DRI"))?);
                }
                0xDA => {
                    header.extend_from_slice(seg);
                    i += 2 + len;
                    break;
                }
                _ => header.extend_from_slice(seg),
            }
            i += 2 + len;
        }
        let sof_at = sof_at.ok_or_else(|| bad("no frame header (SOF)"))?;
        if w == 0 || h == 0 {
            return Err(bad(format!("frame size {w} x {h}")));
        }
        Ok(JpegLayout {
            header,
            sof_at,
            width: w,
            height: h,
            mcu_w,
            mcu_h,
            restart,
            scan_start: i as u64,
        })
    }

    /// Width of a full interval in pixels.
    pub fn interval_width(&self) -> u32 {
        self.restart.saturating_mul(self.mcu_w)
    }

    /// Intervals per MCU row.
    pub fn intervals_per_row(&self) -> u32 {
        let mcus = self.width.div_ceil(self.mcu_w.max(1));
        mcus.div_ceil(self.restart.max(1))
    }

    /// MCU rows.
    pub fn rows(&self) -> u32 {
        self.height.div_ceil(self.mcu_h.max(1))
    }

    /// Width and height in pixels of interval `k` of row `row`.
    pub fn interval_size(&self, row: u32, k: u32) -> (u32, u32) {
        let iw = self.interval_width();
        let w = iw.min(self.width.saturating_sub(k * iw));
        let h = self.mcu_h.min(self.height.saturating_sub(row * self.mcu_h));
        (w, h)
    }

    /// A complete JPEG for one interval of `w × h` pixels: the header with the frame size set,
    /// the interval's bytes without a trailing restart (or end) marker, and an end marker.
    pub fn frame(&self, w: u32, h: u32, interval: &[u8]) -> Vec<u8> {
        let mut body = interval;
        if let [rest @ .., 0xFF, m] = body
            && ((0xD0..=0xD7).contains(m) || *m == 0xD9)
        {
            body = rest;
        }
        let mut out = Vec::with_capacity(self.header.len() + body.len() + 2);
        out.extend_from_slice(&self.header);
        let at = self.sof_at;
        out[at + 5..at + 7].copy_from_slice(&u16::try_from(h).unwrap_or(u16::MAX).to_be_bytes());
        out[at + 7..at + 9].copy_from_slice(&u16::try_from(w).unwrap_or(u16::MAX).to_be_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }
}

/// Split the bytes of one MCU row (from its first entropy-coded byte to the start of the next
/// row, or to the end of the scan) at its restart markers: the byte ranges of its intervals,
/// relative to `bytes`. Byte-stuffed `FF 00` and fill bytes are not markers.
pub fn split_intervals(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0xFF {
            let m = bytes[i + 1];
            if (0xD0..=0xD7).contains(&m) {
                out.push(start..i);
                start = i + 2;
                i += 2;
                continue;
            }
            if m == 0xD9 {
                out.push(start..i);
                return out;
            }
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(start..bytes.len());
    }
    out
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
    fn header_rows_and_intervals() {
        let mut h = vec![0xFF, 0xD8];
        h.extend(seg(0xDB, &[0; 65]));
        let mut sof = vec![8];
        sof.extend_from_slice(&100u16.to_be_bytes());
        sof.extend_from_slice(&9000u16.to_be_bytes());
        sof.extend_from_slice(&[3, 0, 0x11, 0, 1, 0x11, 1, 2, 0x11, 1]);
        h.extend(seg(0xC0, &sof));
        h.extend(seg(0xDD, &512u16.to_be_bytes()));
        h.extend(seg(0xDA, &[3, 0, 0, 1, 0x11, 2, 0x11, 0, 63, 0]));
        let l = JpegLayout::parse(&h).unwrap();
        assert_eq!((l.width, l.height, l.mcu_w, l.mcu_h, l.restart), (9000, 100, 8, 8, 512));
        assert_eq!(l.scan_start, h.len() as u64);
        assert_eq!((l.intervals_per_row(), l.rows()), (3, 13));
        assert_eq!(l.interval_size(12, 2), (9000 - 8192, 4));
        assert!(!l.header.windows(2).any(|w| w == [0xFF, 0xDD]));
        let f = l.frame(808, 4, &[1, 0xFF, 0x00, 0xFF, 0xD3]);
        assert_eq!(&f[f.len() - 5..], &[1, 0xFF, 0x00, 0xFF, 0xD9]);
        let at = l.sof_at;
        assert_eq!(&f[at + 5..at + 9], &[0, 4, 0x03, 0x28]);

        let data = [1, 2, 0xFF, 0xD0, 3, 0xFF, 0x00, 0xFF, 0xD1, 4, 0xFF, 0xD9];
        assert_eq!(split_intervals(&data), vec![0..2, 4..7, 9..10]);

        let mut prog = h.clone();
        let i = prog.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        prog[i + 1] = 0xC2;
        assert_eq!(JpegLayout::parse(&prog).unwrap_err().exit_code(), 6);
        assert_eq!(JpegLayout::parse(b"GIF8").unwrap_err().exit_code(), 4);
    }
}
